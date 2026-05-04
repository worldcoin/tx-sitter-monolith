import { Button, Group, Modal, Select, Stack, TextInput } from '@mantine/core';
import { useForm } from '@mantine/form';
import { notifications } from '@mantine/notifications';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { z } from 'zod';
import type { AdminAuth } from '../api/client';
import type { RelayerResponse, TransactionPriority } from '../api/relayers';
import { sendTransaction, type SendTxRequest } from '../api/transactions';

const addressSchema = z.string().regex(/^0x[a-fA-F0-9]{40}$/, 'Expected 20-byte 0x-prefixed address');
const decimalU256Schema = z.string().regex(/^\d+$/, 'Expected decimal unsigned integer string');
const optionalHexDataSchema = z.string().optional().refine((value) => !value || /^0x([a-fA-F0-9]{2})*$/.test(value), {
  message: 'Expected 0x-prefixed even-length hex bytes',
});
const optionalTxIdSchema = z.string().optional();

const sendTxSchema = z.object({
  to: addressSchema,
  value: decimalU256Schema,
  data: optionalHexDataSchema,
  gasLimit: decimalU256Schema,
  priority: z.enum(['slowest', 'slow', 'regular', 'fast', 'fastest']),
  txId: optionalTxIdSchema,
});

type FormValues = z.infer<typeof sendTxSchema>;

export type SendTransactionModalProps = {
  auth: AdminAuth;
  relayer: RelayerResponse | null;
  opened: boolean;
  onClose: () => void;
};

export function SendTransactionModal({ auth, relayer, opened, onClose }: SendTransactionModalProps) {
  const queryClient = useQueryClient();
  const form = useForm<FormValues>({
    initialValues: {
      to: '',
      value: '0',
      data: '',
      gasLimit: '21000',
      priority: 'regular',
      txId: '',
    },
    validate: (values) => {
      const result = sendTxSchema.safeParse(values);
      if (result.success) return {};

      return Object.fromEntries(
        result.error.issues.map((issue) => [issue.path.join('.'), issue.message]),
      );
    },
  });

  const mutation = useMutation({
    mutationFn: (values: SendTxRequest) => {
      if (!relayer) throw new Error('No relayer selected');
      return sendTransaction(auth, relayer.id, values);
    },
    onSuccess: (response) => {
      notifications.show({ color: 'green', title: 'Transaction created', message: response.txId });
      if (relayer) {
        queryClient.invalidateQueries({ queryKey: ['transactions', auth.username, auth.password, relayer.id] });
      }
      form.reset();
      onClose();
    },
    onError: (error) => {
      notifications.show({ color: 'red', title: 'Send transaction failed', message: error.message });
    },
  });

  function handleSubmit(values: FormValues) {
    const parsed = sendTxSchema.parse(values);
    mutation.mutate({
      to: parsed.to,
      value: parsed.value,
      data: parsed.data || undefined,
      gasLimit: parsed.gasLimit,
      priority: parsed.priority as TransactionPriority,
      txId: parsed.txId || undefined,
    });
  }

  return (
    <Modal opened={opened} onClose={onClose} title={relayer ? `Send transaction from ${relayer.name}` : 'Send transaction'}>
      <form onSubmit={form.onSubmit(handleSubmit)}>
        <Stack>
          <TextInput label="To" placeholder="0x…" required {...form.getInputProps('to')} />
          <TextInput label="Value" description="Decimal wei" required {...form.getInputProps('value')} />
          <TextInput label="Data" description="Optional 0x-prefixed hex bytes" {...form.getInputProps('data')} />
          <TextInput label="Gas limit" description="Decimal gas units" required {...form.getInputProps('gasLimit')} />
          <Select
            label="Priority"
            required
            data={[
              { value: 'slowest', label: 'slowest' },
              { value: 'slow', label: 'slow' },
              { value: 'regular', label: 'regular' },
              { value: 'fast', label: 'fast' },
              { value: 'fastest', label: 'fastest' },
            ]}
            {...form.getInputProps('priority')}
          />
          <TextInput label="Tx ID" description="Optional idempotency key" {...form.getInputProps('txId')} />
          <Group justify="flex-end">
            <Button variant="default" onClick={onClose}>Cancel</Button>
            <Button type="submit" loading={mutation.isPending}>Send</Button>
          </Group>
        </Stack>
      </form>
    </Modal>
  );
}
