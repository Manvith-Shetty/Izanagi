// The person's own wallet app (MetaMask, or anything that injects `window.ethereum`).
// Tab builds every transaction; this only asks the wallet to sign and send it.

interface Eip1193 {
  request(args: { method: string; params?: unknown[] }): Promise<unknown>;
}

const BASE = {
  chainId: "0x2105",
  chainName: "Base",
  nativeCurrency: { name: "Ether", symbol: "ETH", decimals: 18 },
  rpcUrls: ["https://mainnet.base.org"],
  blockExplorerUrls: ["https://basescan.org"],
};

function eth(): Eip1193 {
  const e = (window as unknown as { ethereum?: Eip1193 }).ethereum;
  if (!e) throw new Error("No MetaMask in this browser. Install it from metamask.io, then come back to this page.");
  return e;
}

/** Ask for an account, on Tab's chain. Returns the account's address, lowercased. */
export async function connect(chainId: number): Promise<string> {
  const accounts = (await eth().request({ method: "eth_requestAccounts" })) as string[];
  if (!accounts?.length) throw new Error("MetaMask didn't share an account.");
  const want = `0x${chainId.toString(16)}`;
  const have = (await eth().request({ method: "eth_chainId" })) as string;
  if (have.toLowerCase() !== want) {
    try {
      await eth().request({ method: "wallet_switchEthereumChain", params: [{ chainId: want }] });
    } catch (e) {
      // 4902: MetaMask doesn't know the chain yet
      if ((e as { code?: number }).code === 4902 && want === BASE.chainId) {
        await eth().request({ method: "wallet_addEthereumChain", params: [BASE] });
      } else {
        throw e;
      }
    }
  }
  return accounts[0].toLowerCase();
}

/** Sign and send one transaction Tab built. Returns its hash as soon as MetaMask has sent it. */
export async function send(from: string, tx: { to: string; data: string }): Promise<string> {
  return (await eth().request({
    method: "eth_sendTransaction",
    params: [{ from, to: tx.to, data: tx.data, value: "0x0" }],
  })) as string;
}

/** MetaMask's errors, in words a person can act on. */
export function explain(e: unknown): string {
  const err = e as { code?: number; message?: string };
  if (err.code === 4001) return "You cancelled it in MetaMask. Nothing was sent.";
  if (err.code === -32002) return "MetaMask is already asking you something. Open it and answer that first.";
  return err.message ?? String(e);
}
