import { Code, Modal, ScrollArea, Table, Text } from '@mantine/core';
import type { AdminTxResponse } from '../api/transactions';

export type TransactionDetailModalProps = {
  tx: AdminTxResponse | null;
  opened: boolean;
  onClose: () => void;
};

function stringify(value: unknown): string {
  if (value === undefined || value === null || value === '') return '—';
  if (typeof value === 'object') return JSON.stringify(value, null, 2);
  return String(value);
}

export function TransactionDetailModal({ tx, opened, onClose }: TransactionDetailModalProps) {
  return (
    <Modal opened={opened} onClose={onClose} size="xl" title={tx ? `Transaction ${tx.txId}` : 'Transaction'}>
      {tx ? (
        <ScrollArea h={520}>
          <Table withTableBorder withColumnBorders striped>
            <Table.Tbody>
              {Object.entries(tx).map(([key, value]) => (
                <Table.Tr key={key}>
                  <Table.Th w={180}>{key}</Table.Th>
                  <Table.Td>
                    <Code block>{stringify(value)}</Code>
                  </Table.Td>
                </Table.Tr>
              ))}
              <Table.Tr>
                <Table.Th>from</Table.Th>
                <Table.Td><Text c="dimmed">Not exposed by current API response</Text></Table.Td>
              </Table.Tr>
              <Table.Tr>
                <Table.Th>createdAt</Table.Th>
                <Table.Td><Text c="dimmed">Not exposed by current API response</Text></Table.Td>
              </Table.Tr>
            </Table.Tbody>
          </Table>
        </ScrollArea>
      ) : null}
    </Modal>
  );
}
