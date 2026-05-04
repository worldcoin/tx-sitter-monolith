import { AppShell, Button, Container, Grid, Group, PasswordInput, Stack, Text, TextInput, Title } from '@mantine/core';
import { useDisclosure, useLocalStorage } from '@mantine/hooks';
import { useState } from 'react';
import { API_BASE_URL, type AdminAuth } from './api/client';
import type { RelayerResponse, TxStatus } from './api/relayers';
import type { AdminTxResponse } from './api/transactions';
import { DeleteRelayerModal } from './components/DeleteRelayerModal';
import { RelayerList } from './components/RelayerList';
import { SendTransactionModal } from './components/SendTransactionModal';
import { TransactionDetailModal } from './components/TransactionDetailModal';
import { TransactionTable } from './components/TransactionTable';

function AuthBar({ auth, onChange }: { auth: AdminAuth; onChange: (auth: AdminAuth) => void }) {
  return (
    <Group align="end" gap="sm">
      <TextInput
        label="Admin username"
        value={auth.username}
        onChange={(event) => onChange({ ...auth, username: event.currentTarget.value })}
        w={180}
      />
      <PasswordInput
        label="Admin password"
        value={auth.password}
        onChange={(event) => onChange({ ...auth, password: event.currentTarget.value })}
        w={180}
      />
      <Text size="xs" c="dimmed">API: {API_BASE_URL}</Text>
    </Group>
  );
}

export default function App() {
  const [auth, setAuth] = useLocalStorage<AdminAuth>({
    key: 'tx-sitter-admin-ui-auth',
    defaultValue: { username: 'admin', password: 'admin' },
  });
  const [selectedRelayer, setSelectedRelayer] = useState<RelayerResponse | null>(null);
  const [selectedTx, setSelectedTx] = useState<AdminTxResponse | null>(null);
  const [txStatus, setTxStatus] = useState<TxStatus | 'unsent' | 'all'>('all');
  const [detailOpened, detailHandlers] = useDisclosure(false);
  const [sendOpened, sendHandlers] = useDisclosure(false);
  const [deleteOpened, deleteHandlers] = useDisclosure(false);

  return (
    <AppShell header={{ height: 76 }} padding="md">
      <AppShell.Header>
        <Container fluid py="sm">
          <Group justify="space-between">
            <div>
              <Title order={2}>tx-sitter admin</Title>
              <Text size="xs" c="dimmed">Minimal internal frontend for admin endpoints</Text>
            </div>
            <AuthBar auth={auth} onChange={setAuth} />
          </Group>
        </Container>
      </AppShell.Header>

      <AppShell.Main>
        <Container fluid>
          <Stack>
            <Grid gap="md">
              <Grid.Col span={{ base: 12, lg: 5 }}>
                <RelayerList
                  auth={auth}
                  selectedRelayerId={selectedRelayer?.id}
                  onSelect={(relayer) => {
                    setSelectedRelayer(relayer);
                    setTxStatus('all');
                  }}
                />
              </Grid.Col>
              <Grid.Col span={{ base: 12, lg: 7 }}>
                {selectedRelayer ? (
                  <TransactionTable
                    auth={auth}
                    relayer={selectedRelayer}
                    status={txStatus}
                    onStatusChange={setTxStatus}
                    onView={(tx) => {
                      setSelectedTx(tx);
                      detailHandlers.open();
                    }}
                    onSend={sendHandlers.open}
                    onDelete={deleteHandlers.open}
                  />
                ) : (
                  <Stack align="center" justify="center" h={300}>
                    <Text c="dimmed">Select a relayer to view transactions.</Text>
                    <Button variant="default" disabled>Select relayer</Button>
                  </Stack>
                )}
              </Grid.Col>
            </Grid>
          </Stack>
        </Container>
      </AppShell.Main>

      <TransactionDetailModal tx={selectedTx} opened={detailOpened} onClose={detailHandlers.close} />
      <SendTransactionModal auth={auth} relayer={selectedRelayer} opened={sendOpened} onClose={sendHandlers.close} />
      <DeleteRelayerModal
        auth={auth}
        relayer={selectedRelayer}
        opened={deleteOpened}
        onClose={deleteHandlers.close}
        onDeleted={() => setSelectedRelayer(null)}
      />
    </AppShell>
  );
}
