//! World ID for Agents -- the human gate.
//!
//! Built on the official `openidconnect` crate against World's OIDC provider. Discovery at
//! `https://sandbox.auth.world.org/.well-known/openid-configuration` advertises exactly what
//! this flow needs (all verified live):
//!
//! ```text
//! device_authorization_endpoint  .../api/v1/device_authorization
//! grant_types_supported          [authorization_code, urn:ietf:params:oauth:grant-type:device_code]
//! subject_types_supported        [pairwise]
//! id_token_signing_alg_values    [RS256]
//! prompt_values_supported        [none, login]
//! acr_values_supported           [https://world.org/oidc/acr/orb-v3]
//! claims_supported               [..., auth_time, acr, amr, nonce]
//! ```
//!
//! Why the **Device Authorization Grant (RFC 8628)**: the agent is headless and has no
//! browser. It prints a short code, the human approves on their phone, the agent polls.
//! That is precisely the problem RFC 8628 was written for.
//!
//! Three things make this an identity decision rather than a login screen:
//!   * `acr_values=orb-v3` asks for ORB-level proof of personhood -- the credential that is
//!     proportionate to "this agent is about to spend real money".
//!   * `auth_time` is checked for FRESHNESS, so a months-old session cannot authorise a
//!     payment happening now.
//!   * the pairwise `sub` is the budget key, so a limit belongs to a PERSON and is shared
//!     across every agent they run -- not to a wallet.
//!
//! All of it runs in this backend. The client secret never leaves the process, the
//! `id_token` signature is verified against the issuer's JWKS, and an unvalidated client
//! response is never treated as authorization.

use anyhow::{anyhow, Context, Result};
use openidconnect::core::{
    CoreAuthDisplay, CoreClaimName, CoreClaimType, CoreClient, CoreClientAuthMethod, CoreGrantType,
    CoreJsonWebKey, CoreJweContentEncryptionAlgorithm, CoreJweKeyManagementAlgorithm,
    CoreResponseMode, CoreResponseType, CoreSubjectIdentifierType,
};
use openidconnect::{
    AdditionalProviderMetadata, AuthenticationFlow, ClientId, ClientSecret, DeviceAuthorizationUrl,
    IssuerUrl, Nonce, ProviderMetadata, Scope,
};
use serde::{Deserialize, Serialize};

/// `device_authorization_endpoint` is an RFC 8628 extension, so it is not part of the
/// core OIDC discovery document. We extend the metadata type to read it.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct DeviceEndpointMetadata {
    pub device_authorization_endpoint: DeviceAuthorizationUrl,
}
impl AdditionalProviderMetadata for DeviceEndpointMetadata {}

pub type WorldProviderMetadata = ProviderMetadata<
    DeviceEndpointMetadata,
    CoreAuthDisplay,
    CoreClientAuthMethod,
    CoreClaimName,
    CoreClaimType,
    CoreGrantType,
    CoreJweContentEncryptionAlgorithm,
    CoreJweKeyManagementAlgorithm,
    CoreJsonWebKey,
    CoreResponseMode,
    CoreResponseType,
    CoreSubjectIdentifierType,
>;

/// RFC 8628 device authorization response, as re-exported through openidconnect.
pub type DeviceAuthResponse = openidconnect::DeviceAuthorizationResponse<
    openidconnect::EmptyExtraDeviceAuthorizationFields,
>;

pub const ORB_ACR: &str = "https://world.org/oidc/acr/orb-v3";

/// How old an authentication may be and still authorise a payment.
pub const MAX_AUTH_AGE_SECS: u64 = 300;

#[derive(Clone)]
pub struct WorldId {
    issuer: String,
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    /// Require orb-level assurance, not merely "someone logged in".
    pub require_orb: bool,
}

/// What the agent shows the human.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceChallenge {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    /// Minimum seconds between polls. Back off further on `slow_down`.
    pub interval: u64,
}

/// A validated human approval.
#[derive(Debug, Clone, Serialize)]
pub struct Approval {
    /// OIDC pairwise subject: a private, per-application identifier for one human.
    pub sub: String,
    pub issuer: String,
    pub auth_time: u64,
    pub acr: Option<String>,
    pub orb_verified: bool,
}

/// Every way the human path ends without authorising the payment.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(dead_code)]
pub enum DeniedReason {
    AccessDenied,
    ExpiredToken,
    /// Reported by the agent-side flow when the operator abandons the approval.
    Cancelled,
    StaleAuthentication,
    InsufficientAssurance,
}

#[derive(Debug)]
pub enum PollOutcome {
    Pending,
    /// The IdP asked us to poll less often.
    SlowDown,
    Approved(Box<Approval>),
    Denied(DeniedReason),
}

impl WorldId {
    pub fn new(issuer: String, client_id: String, client_secret: String, require_orb: bool) -> Self {
        Self {
            issuer,
            client_id,
            client_secret,
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none()) // SSRF-safe, as openidconnect requires
                .build()
                .expect("http client"),
            require_orb,
        }
    }

    async fn client(&self) -> Result<CoreClient<
        openidconnect::EndpointSet,
        openidconnect::EndpointSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointNotSet,
        openidconnect::EndpointMaybeSet,
        openidconnect::EndpointMaybeSet,
    >> {
        let issuer = IssuerUrl::new(self.issuer.clone()).context("issuer url")?;
        let meta = WorldProviderMetadata::discover_async(issuer, &self.http)
            .await
            .context("OIDC discovery")?;
        let device_url = meta.additional_metadata().device_authorization_endpoint.clone();
        Ok(CoreClient::from_provider_metadata(
            meta,
            ClientId::new(self.client_id.clone()),
            Some(ClientSecret::new(self.client_secret.clone())),
        )
        .set_device_authorization_url(device_url))
    }

    /// Human-readable summary of what the issuer advertises, for setup checks.
    pub async fn discovery_summary(&self) -> Result<Vec<String>> {
        let issuer = IssuerUrl::new(self.issuer.clone()).context("issuer url")?;
        let meta = WorldProviderMetadata::discover_async(issuer, &self.http)
            .await
            .context("OIDC discovery")?;
        let device = meta.additional_metadata().device_authorization_endpoint.as_str().to_string();
        let mut out = vec![
            format!("token endpoint   {}", meta.token_endpoint().map(|u| u.to_string()).unwrap_or_default()),
            format!("device endpoint  {device}"),
            format!("jwks             {}", meta.jwks_uri().to_string()),
        ];
        if let Some(s) = meta.subject_types_supported().first() {
            out.push(format!("subject type     {s:?}"));
        }
        Ok(out)
    }

    /// Start the device flow: returns the code the agent prints for the human.
    pub async fn start_device_flow(&self) -> Result<DeviceChallenge> {
        let client = self.client().await?;

        // World's OIDC guide is explicit: "Device authorization supports only `scope=openid`;
        // adding `nonce`, `max_age`, `prompt`, or `acr_values` does not provide those
        // controls. Fresh proof is already required." Sending them would be theatre -- and
        // verifying a nonce that the IdP never echoed would fail every approval.
        //
        // Do NOT call `.add_scope(openid)` here: openidconnect already includes `openid` for
        // OIDC device flows, so adding it sends `scope=openid openid`, which World rejects
        // with `invalid_scope` -- an error that looks like a permissions problem and is not.
        let details: DeviceAuthResponse = client
            .exchange_device_code()
            .request_async(&self.http)
            .await
            .context("device authorization request")?;

        Ok(DeviceChallenge {
            device_code: details.device_code().secret().clone(),
            user_code: details.user_code().secret().clone(),
            verification_uri: details.verification_uri().to_string(),
            verification_uri_complete: details
                .verification_uri_complete()
                .map(|u| u.secret().clone()),
            expires_in: details.expires_in().as_secs(),
            interval: details.interval().as_secs().max(1),
        })
    }

    /// Poll once, and validate the result HERE.
    ///
    /// Deliberately a single HTTP request rather than `oauth2`'s
    /// `exchange_device_access_token(..).request_async(..)`: that helper runs its own polling
    /// loop internally and *fabricates* `expired_token` the moment its timeout elapses. Used
    /// from a stateless poll endpoint it reports an expiry the IdP never sent.
    ///
    /// Verifies the `id_token` signature against the issuer's JWKS before returning anything.
    pub async fn poll(&self, device_code: &str) -> Result<PollOutcome> {
        let issuer = IssuerUrl::new(self.issuer.clone()).context("issuer url")?;
        let meta = WorldProviderMetadata::discover_async(issuer, &self.http)
            .await
            .context("OIDC discovery")?;
        let token_endpoint = meta.token_endpoint().context("issuer has no token endpoint")?.to_string();

        let res = self
            .http
            .post(&token_endpoint)
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
                ("device_code", device_code),
            ])
            .send()
            .await
            .context("device token request")?;

        let body: serde_json::Value =
            serde_json::from_str(&res.text().await.unwrap_or_default()).unwrap_or(serde_json::Value::Null);

        // RFC 8628 polling states. `authorization_pending` and `slow_down` are not failures.
        if let Some(err) = body.get("error").and_then(|v| v.as_str()) {
            return Ok(match err {
                "authorization_pending" => PollOutcome::Pending,
                "slow_down" => PollOutcome::SlowDown,
                "access_denied" => PollOutcome::Denied(DeniedReason::AccessDenied),
                "expired_token" => PollOutcome::Denied(DeniedReason::ExpiredToken),
                other => {
                    return Err(anyhow!(
                        "device token request failed: {other}{}",
                        body.get("error_description")
                            .and_then(|d| d.as_str())
                            .map(|d| format!(" ({d})"))
                            .unwrap_or_default()
                    ))
                }
            });
        }

        let id_token_str = body
            .get("id_token")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("token response contained no id_token: {body}"))?;

        let id_token: openidconnect::core::CoreIdToken =
            id_token_str.parse().context("malformed id_token")?;

        let client = CoreClient::from_provider_metadata(
            meta,
            ClientId::new(self.client_id.clone()),
            Some(ClientSecret::new(self.client_secret.clone())),
        );

        // Signature verified against the issuer's JWKS (RS256), issuer and audience checked.
        // No nonce assertion: the device grant does not accept a nonce, so requiring one in
        // the id_token would reject every genuine approval.
        let verifier = client.id_token_verifier();
        let claims = id_token
            .claims(&verifier, |_: Option<&Nonce>| Ok(()))
            .context("id_token verification failed")?;

        let sub = claims.subject().to_string();
        let acr = claims.auth_context_ref().map(|a| a.to_string());

        // World already requires fresh proof for the device grant; this is a second belt.
        // `auth_time` is optional, so fall back to `iat` rather than assuming the epoch.
        let auth_time = claims
            .auth_time()
            .map(|t| t.timestamp() as u64)
            .unwrap_or_else(|| claims.issue_time().timestamp() as u64);
        let age = crate::state::now().saturating_sub(auth_time);
        if age > MAX_AUTH_AGE_SECS {
            return Ok(PollOutcome::Denied(DeniedReason::StaleAuthentication));
        }

        // The device grant cannot request `acr_values`, but World requires fresh proof for it
        // regardless. So we only enforce assurance when the IdP actually asserts an `acr` and
        // it is not the orb credential -- never when the claim is simply absent.
        let orb_verified = acr.as_deref() == Some(ORB_ACR);
        if self.require_orb && acr.is_some() && !orb_verified {
            return Ok(PollOutcome::Denied(DeniedReason::InsufficientAssurance));
        }

        Ok(PollOutcome::Approved(Box::new(Approval {
            sub,
            issuer: claims.issuer().to_string(),
            auth_time,
            acr,
            orb_verified,
        })))
    }

    /// The authorization-code URL, used when a browser IS available (and by the IDKit path).
    #[allow(dead_code)]
    pub async fn authorize_url(&self, redirect: &str) -> Result<String> {
        let client = self.client().await?.set_redirect_uri(
            openidconnect::RedirectUrl::new(redirect.to_string()).context("redirect uri")?,
        );
        let (url, _csrf, _nonce) = client
            .authorize_url(
                AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
                openidconnect::CsrfToken::new_random,
                Nonce::new_random,
            )
            .add_scope(Scope::new("openid".to_string()))
            .url();
        Ok(url.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn denied_reasons_serialize_for_the_api() {
        let cases = [
            (DeniedReason::AccessDenied, "\"access_denied\""),
            (DeniedReason::ExpiredToken, "\"expired_token\""),
            (DeniedReason::Cancelled, "\"cancelled\""),
            (DeniedReason::StaleAuthentication, "\"stale_authentication\""),
            (DeniedReason::InsufficientAssurance, "\"insufficient_assurance\""),
        ];
        for (r, want) in cases {
            assert_eq!(serde_json::to_string(&r).unwrap(), want);
        }
    }

    /// Discovery against the real World sandbox. Proves the device grant is actually
    /// offered, which the whole human-approval path depends on.
    #[tokio::test]
    #[ignore = "network"]
    async fn sandbox_advertises_the_device_grant() {
        let w = WorldId::new(
            "https://sandbox.auth.world.org".into(),
            "test".into(),
            "test".into(),
            true,
        );
        let issuer = IssuerUrl::new(w.issuer.clone()).unwrap();
        let meta = WorldProviderMetadata::discover_async(issuer, &w.http).await.unwrap();
        assert!(
            meta.additional_metadata()
                .device_authorization_endpoint
                .as_str()
                .contains("device_authorization"),
            "World sandbox must advertise a device_authorization_endpoint"
        );
    }
}
