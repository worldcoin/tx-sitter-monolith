use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy::eips::eip2718::Encodable2718;
use alloy::network::TxSigner;
use alloy::primitives::{Address, Bytes};
use alloy::signers::aws::AwsSigner;
use alloy::signers::local::PrivateKeySigner;

#[derive(Debug)]
pub enum UniversalSigner {
    Aws(AwsSigner),
    Local(PrivateKeySigner),
}

impl UniversalSigner {
    pub fn address(&self) -> Address {
        match self {
            Self::Aws(signer) => signer.address(),
            Self::Local(signer) => signer.address(),
        }
    }

    /// Signs `tx` and returns its EIP-2718 encoding, the payload of
    /// `eth_sendRawTransaction`.
    ///
    /// The signature commits to the chain id set on `tx`.
    pub async fn raw_signed_tx(
        &self,
        mut tx: TxEip1559,
    ) -> eyre::Result<Bytes> {
        let signature = match self {
            Self::Aws(signer) => signer.sign_transaction(&mut tx).await?,
            Self::Local(signer) => signer.sign_transaction(&mut tx).await?,
        };

        let signed_tx = TxEnvelope::from(tx.into_signed(signature));

        Ok(signed_tx.encoded_2718().into())
    }
}
