import { Badge, Button, Group, Paper, ScrollArea, Select, Table, Text, Title } from '@mantine/core';
import { useQuery } from '@tanstack/react-query';
import type { AdminAuth } from '../api/client';
import type { RelayerResponse, TxStatus } from '../api/relayers';
import { listTransactions, type AdminTxResponse } from '../api/transactions';

function compact(value?: string, head = 10, tail = 6): string {
  if (!value) return '—';
  if (value.length <= head + tail + 3) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

function statusColor(status?: TxStatus): string {
  if (!status) return 'yellow';
  if (status === 'finalized') return 'green';
  if (status === 'mined') return 'blue';
  return 'orange';
}

export type TransactionTableProps = {
  auth: AdminAuth;
  relayer: RelayerResponse;
  status?: TxStatus | 'unsent' | 'all';
  onStatusChange: (value: TxStatus | 'unsent' | 'all') => void;
  onView: (tx: AdminTxResponse) => void;
  onSend: () => void;
  onDelete: () => void;
};

export function TransactionTable({
  auth,
  relayer,
  status = 'all',
  onStatusChange,
  onView,
  onSend,
  onDelete,
}: TransactionTableProps) {
  const txsQuery = useQuery({
    queryKey: ['transactions', auth.username, auth.password, relayer.id, status],
    queryFn: () => listTransactions(auth, relayer.id, status === 'all' ? {} : status === 'unsent' ? { unsent: true } : { status }),
  });

  const txs = txsQuery.data || [];

  return (
    <Paper withBorder p="md">
      <Group justify="space-between" mb="sm" align="flex-start">
        <div>
          <Title order={3}>Relayer detail</Title>
          <Text size="sm"><b>{relayer.name}</b> · <Text span ff="monospace">{relayer.id}</Text></Text>
          <Text size="xs" c="dimmed">Address: <Text span ff="monospace">{relayer.address}</Text></Text>
        </div>
        <Group>
          <Select
            label="Status"
            value={status}
            onChange={(value) => onStatusChange((value || 'all') as TxStatus | 'unsent' | 'all')}
            data={[
              { value: 'all', label: 'All' },
              { value: 'unsent', label: 'Unsent' },
              { value: 'pending', label: 'Pending' },
              { value: 'mined', label: 'Mined' },
              { value: 'finalized', label: 'Finalized' },
            ]}
            w={140}
          />
          <Button mt={25} onClick={() => txsQuery.refetch()} loading={txsQuery.isFetching} variant="default">Refresh</Button>
          <Button mt={25} onClick={onSend}>Send tx</Button>
          <Button mt={25} onClick={onDelete} color="red" variant="outline">Delete relayer</Button>
        </Group>
      </Group>

      {txsQuery.error ? <Text c="red" size="sm">{txsQuery.error.message}</Text> : null}

      <ScrollArea h={520}>
        <Table striped highlightOnHover withTableBorder withColumnBorders stickyHeader>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>Tx ID</Table.Th>
              <Table.Th>Status</Table.Th>
              <Table.Th>To</Table.Th>
              <Table.Th>From</Table.Th>
              <Table.Th>Value</Table.Th>
              <Table.Th>Gas</Table.Th>
              <Table.Th>Created at</Table.Th>
              <Table.Th>Nonce</Table.Th>
              <Table.Th>Hash</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {txs.map((tx) => (
              <Table.Tr key={tx.txId} onClick={() => onView(tx)} style={{ cursor: 'pointer' }}>
                <Table.Td><Text size="xs" ff="monospace">{compact(tx.txId, 14, 8)}</Text></Table.Td>
                <Table.Td><Badge color={statusColor(tx.status)}>{tx.status || 'unsent'}</Badge></Table.Td>
                <Table.Td><Text size="xs" ff="monospace">{compact(tx.to)}</Text></Table.Td>
                <Table.Td><Text size="xs" c="dimmed">not exposed</Text></Table.Td>
                <Table.Td><Text size="xs" ff="monospace">{tx.value}</Text></Table.Td>
                <Table.Td><Text size="xs" ff="monospace">{tx.gasLimit}</Text></Table.Td>
                <Table.Td><Text size="xs" c="dimmed">not exposed</Text></Table.Td>
                <Table.Td>{tx.nonce}</Table.Td>
                <Table.Td><Text size="xs" ff="monospace">{compact(tx.txHash)}</Text></Table.Td>
              </Table.Tr>
            ))}
            {!txsQuery.isLoading && txs.length === 0 ? (
              <Table.Tr><Table.Td colSpan={9}><Text c="dimmed">No transactions</Text></Table.Td></Table.Tr>
            ) : null}
          </Table.Tbody>
        </Table>
      </ScrollArea>
    </Paper>
  );
}
