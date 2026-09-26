// The owner's own wallet, browser-side only. Izanagi never sees the owner's key: the owner signs
// the emergency brake (pause) and on-chain revocations themselves.

import { createConfig, http } from "wagmi";
import { base } from "wagmi/chains";
import { coinbaseWallet, injected } from "wagmi/connectors";
import { defineChain } from "viem";

const chainId = Number(import.meta.env.VITE_CHAIN_ID ?? base.id);
const rpc = import.meta.env.VITE_RPC_URL || base.rpcUrls.default.http[0];

/** Base, or a local fork of it answering on VITE_RPC_URL. */
export const chain =
  chainId === base.id && !/127\.0\.0\.1|localhost/.test(rpc)
    ? base
    : defineChain({ ...base, id: chainId, name: "Base (local fork)", rpcUrls: { default: { http: [rpc] } } });

export const explorer = (import.meta.env.VITE_EXPLORER_URL as string | undefined) || "https://basescan.org";

export const wagmiConfig = createConfig({
  chains: [chain],
  connectors: [
    injected({ target: "metaMask" }),
    coinbaseWallet({ appName: "Izanagi", preference: { options: "smartWalletOnly" } }),
  ],
  transports: { [chain.id]: http(rpc) },
});

declare module "wagmi" {
  interface Register {
    config: typeof wagmiConfig;
  }
}

/** The slice of the wallet contract the owner can call from here. */
export const walletAbi = [
  { type: "function", name: "owner", stateMutability: "view", inputs: [], outputs: [{ type: "address" }] },
  { type: "function", name: "paused", stateMutability: "view", inputs: [], outputs: [{ type: "bool" }] },
  { type: "function", name: "setPaused", stateMutability: "nonpayable", inputs: [{ name: "p", type: "bool" }], outputs: [] },
  {
    type: "function",
    name: "revoke",
    stateMutability: "nonpayable",
    inputs: [
      { name: "seller", type: "address" },
      { name: "reason", type: "uint32" },
    ],
    outputs: [],
  },
] as const;
