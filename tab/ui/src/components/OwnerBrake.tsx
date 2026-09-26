import { useAccount, useConnect, useDisconnect, useReadContract, useWaitForTransactionReceipt, useWriteContract } from "wagmi";
import type { Address } from "viem";
import { explorer, walletAbi } from "../lib/wallet";
import { short } from "../lib/format";

// The emergency brake, signed by the owner's own wallet. It goes straight to the chain, so it
// works even when Izanagi's server is down.
export function OwnerBrake({ wallet, sample }: { wallet: string; sample: boolean }) {
  const { address, isConnected } = useAccount();
  const { connectors, connect, isPending: connecting, error: connectError } = useConnect();
  const { disconnect } = useDisconnect();
  const target = wallet as Address;
  const enabled = !sample && /^0x[0-9a-fA-F]{40}$/.test(wallet);
  const owner = useReadContract({ address: target, abi: walletAbi, functionName: "owner", query: { enabled } });
  const paused = useReadContract({ address: target, abi: walletAbi, functionName: "paused", query: { enabled, refetchInterval: 5000 } });
  const { writeContract, data: hash, isPending: signing, error: writeError, reset } = useWriteContract();
  const receipt = useWaitForTransactionReceipt({ hash });

  const isOwner = !!address && !!owner.data && owner.data.toLowerCase() === address.toLowerCase();
  const isPaused = paused.data === true;

  const toggle = () => {
    reset();
    writeContract(
      { address: target, abi: walletAbi, functionName: "setPaused", args: [!isPaused] },
      { onSuccess: () => setTimeout(() => paused.refetch(), 2500) },
    );
  };

  const err = (writeError ?? connectError)?.message.split("\n")[0];

  return (
    <section aria-labelledby="brake" className="surface-raised rounded-2xl p-r4 md:p-r5">
      <div className="flex flex-wrap items-baseline gap-x-r3">
        <h2 id="brake" className="display text-[1.4rem] tracking-[-0.02em]">
          Your own brake
        </h2>
        {paused.data !== undefined && (
          <span className={`text-[0.85rem] font-semibold ${isPaused ? "text-beni" : "text-tide"}`}>
            {isPaused ? "Wallet paused" : "Wallet active"}
          </span>
        )}
      </div>
      <p className="mt-r2 max-w-[36rem] text-[0.95rem] text-sumi-soft">
        Pause the wallet from your own wallet and no voucher can be claimed by anyone, including ones already signed. This goes straight to
        the chain, so it works even if Izanagi is down.
      </p>

      <div className="mt-r4 flex flex-wrap items-center gap-r3">
        {!isConnected ? (
          connectors.map((c) => (
            <button key={c.uid} type="button" className="btn btn-quiet" disabled={connecting} onClick={() => connect({ connector: c })}>
              {c.type === "coinbaseWallet" ? "Coinbase Smart Wallet" : "MetaMask"}
            </button>
          ))
        ) : isOwner ? (
          <button type="button" className={`btn ${isPaused ? "btn-primary" : "btn-seal"}`} disabled={signing || receipt.isLoading} onClick={toggle}>
            {signing ? "Confirm in your wallet" : receipt.isLoading ? "Waiting for the block" : isPaused ? "Resume payments" : "Pause every payment"}
          </button>
        ) : (
          <p className="text-[0.92rem] text-sumi-soft">
            <span className="hex">{short(address)}</span> isn't this wallet's owner
            {owner.data ? (
              <>
                {" "}
                (<span className="hex">{short(owner.data)}</span> is)
              </>
            ) : null}
            . Connect the owner's wallet to use the brake.
          </p>
        )}
        {isConnected && (
          <button type="button" className="btn btn-ghost text-[0.88rem]" onClick={() => disconnect()}>
            Disconnect <span className="hex">{short(address)}</span>
          </button>
        )}
      </div>

      <div aria-live="polite" className="mt-r3 text-[0.88rem]">
        {sample && !isConnected && <p className="text-stone">The brake needs your real wallet contract. Start the Tab server to use it.</p>}
        {hash && (
          <p className="text-sumi-soft">
            {receipt.isSuccess ? "Done. " : "Sent. "}
            <a className="link" href={`${explorer}/tx/${hash}`} target="_blank" rel="noreferrer">
              View the transaction
            </a>
          </p>
        )}
        {err && <p className="text-beni-deep">{err}</p>}
      </div>
    </section>
  );
}
