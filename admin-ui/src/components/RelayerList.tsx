import { Badge, Button, Group, Paper, ScrollArea, Stack, Table, Text, Title } from '@mantine/core';
import { useQueries, useQuery } from '@tanstack/react-query';
import type { AdminAuth } from '../api/client';
import {
  fetchRelayerBalanceWei,
  listNetworks,
  listRelayers,
  type NetworkResponse,
  type RelayerResponse,
} from '../api/relayers';

function compact(value: string, head = 10, tail = 6): string {
  if (value.length <= head + tail + 3) return value;
  return `${value.slice(0, head)}…${value.slice(-tail)}`;
}

function balanceLabel(balance?: string | null): string {
  if (balance === undefined) return 'loading…';
  if (balance === null) return 'unavailable';
  return `${balance} wei`;
}

export type RelayerListProps = {
  auth: AdminAuth;
  selectedRelayerId?: string;
  onSelect: (relayer: RelayerResponse) => void;
};

export function RelayerList({ auth, selectedRelayerId, onSelect }: RelayerListProps) {
  const relayersQuery = useQuery({
    queryKey: ['relayers', auth.username, auth.password],
    queryFn: () => listRelayers(auth),
  });

  const networksQuery = useQuery({
    queryKey: ['networks', auth.username, auth.password],
    queryFn: () => listNetworks(auth),
  });

  const networksByChainId = new Map<number, NetworkResponse>(
    (networksQuery.data || []).map((network) => [network.chainId, network]),
  );

  const relayers = relayersQuery.data || [];
  const balanceQueries = useQueries({
    queries: relayers.map((relayer) => ({
      queryKey: ['relayer-balance', relayer.id, relayer.address, relayer.chainId, networksByChainId.get(relayer.chainId)?.httpRpc],
      queryFn: () => fetchRelayerBalanceWei(relayer, networksByChainId.get(relayer.chainId)),
      enabled: networksQuery.isSuccess,
      retry: false,
      staleTime: 15_000,
    })),
  });

  return (
    <Paper withBorder p="md">
      <Group justify="space-between" mb="sm">
        <Title order={3}>Relayers</Title>
        <Button size="xs" variant="default" onClick={() => relayersQuery.refetch()} loading={relayersQuery.isFetching}>
          Refresh
        </Button>
      </Group>

      {relayersQuery.error ? (
        <Text c="red" size="sm">{relayersQuery.error.message}</Text>
      ) : null}

      <ScrollArea h={420}>
        <Table striped highlightOnHover withTableBorder withColumnBorders stickyHeader>
          <Table.Thead>
            <Table.Tr>
              <Table.Th>ID</Table.Th>
              <Table.Th>Name</Table.Th>
              <Table.Th>Enabled</Table.Th>
              <Table.Th>Chain</Table.Th>
              <Table.Th>Address</Table.Th>
              <Table.Th>Current balance</Table.Th>
              <Table.Th>Nonce</Table.Th>
              <Table.Th>Current nonce</Table.Th>
            </Table.Tr>
          </Table.Thead>
          <Table.Tbody>
            {relayers.map((relayer, index) => (
              <Table.Tr
                key={relayer.id}
                onClick={() => onSelect(relayer)}
                style={{
                  cursor: 'pointer',
                  background: relayer.id === selectedRelayerId ? 'var(--mantine-color-blue-light)' : undefined,
                }}
              >
                <Table.Td><Text size="xs" ff="monospace">{compact(relayer.id)}</Text></Table.Td>
                <Table.Td>{relayer.name}</Table.Td>
                <Table.Td>
                  <Badge color={relayer.enabled ? 'green' : 'gray'}>{relayer.enabled ? 'yes' : 'no'}</Badge>
                </Table.Td>
                <Table.Td>{relayer.chainId}</Table.Td>
                <Table.Td><Text size="xs" ff="monospace">{compact(relayer.address)}</Text></Table.Td>
                <Table.Td>
                  <Stack gap={2}>
                    <Text size="xs" ff="monospace">
                      {balanceQueries[index]?.isError ? 'unavailable' : balanceLabel(balanceQueries[index]?.data)}
                    </Text>
                    {balanceQueries[index]?.isError ? <Text size="10px" c="dimmed">RPC/CORS failed</Text> : null}
                  </Stack>
                </Table.Td>
                <Table.Td>{relayer.nonce}</Table.Td>
                <Table.Td>{relayer.currentNonce}</Table.Td>
              </Table.Tr>
            ))}
            {!relayersQuery.isLoading && relayers.length === 0 ? (
              <Table.Tr><Table.Td colSpan={8}><Text c="dimmed">No relayers</Text></Table.Td></Table.Tr>
            ) : null}
          </Table.Tbody>
        </Table>
      </ScrollArea>
    </Paper>
  );
}
