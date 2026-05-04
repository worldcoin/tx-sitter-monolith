import { request, type AdminAuth } from './client';

export type DecimalU256 = string;
export type Address = string;
export type H256 = string;

export type TransactionPriority = 'slowest' | 'slow' | 'regular' | 'fast' | 'fastest';
export type TxStatus = 'pending' | 'mined' | 'finalized';

export type RelayerGasPriceLimitResponse = {
  value: DecimalU256;
  chainId: number;
};

export type RelayerResponse = {
  id: string;
  name: string;
  chainId: number;
  keyId: string;
  address: Address;
  nonce: number;
  currentNonce: number;
  maxInflightTxs: number;
  maxQueuedTxs: number;
  gasPriceLimits: RelayerGasPriceLimitResponse[];
  enabled: boolean;
};

export type NetworkResponse = {
  chainId: number;
  name: string;
  httpRpc: string;
  wsRpc: string;
};

export type DeleteRelayerResponse = {
  drainTxHash?: H256;
};

export async function listRelayers(auth: AdminAuth): Promise<RelayerResponse[]> {
  return request<RelayerResponse[]>('/1/admin/relayers', { method: 'GET', auth });
}

export async function getRelayer(auth: AdminAuth, relayerId: string): Promise<RelayerResponse> {
  return request<RelayerResponse>(`/1/admin/relayer/${encodeURIComponent(relayerId)}`, {
    method: 'GET',
    auth,
  });
}

export async function listNetworks(auth: AdminAuth): Promise<NetworkResponse[]> {
  return request<NetworkResponse[]>('/1/admin/networks', { method: 'GET', auth });
}

export async function deleteRelayer(
  auth: AdminAuth,
  relayerId: string,
  drainTo: Address,
): Promise<DeleteRelayerResponse> {
  return request<DeleteRelayerResponse>(`/1/api/admin/relayer/${encodeURIComponent(relayerId)}`, {
    method: 'DELETE',
    auth,
    body: JSON.stringify({ drainTo }),
  });
}

export async function fetchRelayerBalanceWei(relayer: RelayerResponse, network?: NetworkResponse): Promise<string | null> {
  if (!network?.httpRpc) return null;

  const response = await fetch(network.httpRpc, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Accept: 'application/json' },
    body: JSON.stringify({
      jsonrpc: '2.0',
      id: 1,
      method: 'eth_getBalance',
      params: [relayer.address, 'latest'],
    }),
  });

  if (!response.ok) throw new Error(`RPC balance request failed with HTTP ${response.status}`);
  const body = (await response.json()) as { result?: string; error?: { message?: string } };
  if (body.error) throw new Error(body.error.message || 'RPC balance request failed');
  if (!body.result) return null;

  return BigInt(body.result).toString(10);
}
