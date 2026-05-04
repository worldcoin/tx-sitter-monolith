import { Alert, Button, Group, Modal, Stack, Text, TextInput } from '@mantine/core';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import type { AdminAuth } from '../api/client';
import { deleteRelayer, type RelayerResponse } from '../api/relayers';

const deleteSchema = z.object({
  drainTo: z.string().regex(/^0x[a-fA-F0-9]{40}$/, 'Expected 20-byte 0x-prefixed address'),
});

type FormValues = z.infer<typeof deleteSchema>;

export type DeleteRelayerModalProps = {
  auth: AdminAuth;
  relayer: RelayerResponse | null;
  opened: boolean;
  onClose: () => void;
  onDeleted: () => void;
};

export function DeleteRelayerModal({ auth, relayer, opened, onClose, onDeleted }: DeleteRelayerModalProps) {
  const queryClient = useQueryClient();
  const form = useForm<FormValues>({
    initialValues: { drainTo: '' },
    validate: (values) => {
      const result = deleteSchema.safeParse(values);
      if (result.success) return {};
      return Object.fromEntries(result.error.issues.map((issue) => [issue.path.join('.'), issue.message]));
    },
  });

  const mutation = useMutation({
    mutationFn: ({ drainTo }: FormValues) => {
      if (!relayer) throw new Error('No relayer selected');
      return deleteRelayer(auth, relayer.id, drainTo);
    },
    onSuccess: (response) => {
      notifications.show({
        color: 'green',
        title: 'Relayer deleted',
        message: response.drainTxHash ? `Drain tx: ${response.drainTxHash}` : 'No drain transaction was needed',
      });
      queryClient.invalidateQueries({ queryKey: ['relayers', auth.username, auth.password] });
      form.reset();
      onDeleted();
      onClose();
    },
    onError: (error) => {
      notifications.show({ color: 'red', title: 'Delete relayer failed', message: error.message });
    },
  });

  return (
    <Modal opened={opened} onClose={onClose} title={relayer ? `Delete ${relayer.name}` : 'Delete relayer'}>
      <form onSubmit={form.onSubmit((values) => mutation.mutate(values))}>
        <Stack>
          <Alert color="red" title="Destructive action">
            This disables the relayer, purges unsent transactions, drains remaining funds when possible, and deletes it.
          </Alert>
          {relayer ? (
            <Text size="sm">Relayer address: <Text span ff="monospace">{relayer.address}</Text></Text>
          ) : null}
          <TextInput label="drainTo wallet address" placeholder="0x…" required {...form.getInputProps('drainTo')} />
          <Group justify="flex-end">
            <Button variant="default" onClick={onClose}>Cancel</Button>
            <Button type="submit" color="red" loading={mutation.isPending}>Delete relayer</Button>
          </Group>
        </Stack>
      </form>
    </Modal>
  );
}
