import { request, toQueryString, type AdminAuth } from './client';
import type { Address, DecimalU256, H256, TransactionPriority, TxStatus } from './relayers';

export type AdminTxResponse = {
  txId: string;
  relayerId: string;
  to: Address;
  data?: string;
  value: DecimalU256;
  gasLimit: DecimalU256;
  priority: TransactionPriority;
  nonce: number;
  blobs?: string[];
  txHash?: H256;
  status?: TxStatus;
};

export type SendTxRequest = {
  to: Address;
  value: DecimalU256;
  data?: string;
  gasLimit: DecimalU256;
  priority?: TransactionPriority;
  txId?: string;
};

export type SendTxResponse = {
  txId: string;
};

export async function listTransactions(
  auth: AdminAuth,
  relayerId: string,
  filters: { status?: TxStatus; unsent?: boolean } = {},
): Promise<AdminTxResponse[]> {
  const query = toQueryString({ status: filters.status, unsent: filters.unsent });
  return request<AdminTxResponse[]>(
    `/1/api/admin/relayer/${encodeURIComponent(relayerId)}/txs${query}`,
    { method: 'GET', auth },
  );
}

export async function getTransaction(
  auth: AdminAuth,
  relayerId: string,
  txId: string,
): Promise<AdminTxResponse> {
  return request<AdminTxResponse>(
    `/1/api/admin/relayer/${encodeURIComponent(relayerId)}/tx/${encodeURIComponent(txId)}`,
    { method: 'GET', auth },
  );
}

export async function sendTransaction(
  auth: AdminAuth,
  relayerId: string,
  tx: SendTxRequest,
): Promise<SendTxResponse> {
  const payload = Object.fromEntries(
    Object.entries(tx).filter(([, value]) => value !== undefined && value !== ''),
  );

  return request<SendTxResponse>(`/1/api/admin/relayer/${encodeURIComponent(relayerId)}/tx`, {
    method: 'POST',
    auth,
    body: JSON.stringify(payload),
  });
}
