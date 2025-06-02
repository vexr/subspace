//! Code in this file is copied from the frontier repository. Full path is:
//! https://github.com/subspace/frontier/blob/1c667eb43c3d087ac66dc9ed0aa44128373f5b0a/frame/ethereum/src/mock.rs
//! If monorepo points to new commit, this file need to be in sync.
//!
//! Minor changes were made to comments, derives, visibility, and the address_build() seed size.

pub use ethereum::{
    AccessListItem, BlockV2 as Block, LegacyTransactionMessage, Log, ReceiptV3 as Receipt,
    TransactionAction, TransactionSignature, TransactionV2 as Transaction,
};
use frame_support::parameter_types;
use rlp::RlpStream;
use sp_core::crypto::AccountId32;
use sp_core::{H160, H256, U256, keccak_256};

parameter_types! {
    // `490000` is the genesis evm domain chain id
    pub const ChainId: u64 = 490000;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountInfo {
    pub address: H160,
    pub account_id: AccountId32,
    pub private_key: H256,
}

/// Returns an AccountInfo struct with a deterministic address and private key.
/// Seed zero produces an invalid private key, so we add one to the supplied seed.
pub fn address_build(mut seed: u128) -> AccountInfo {
    seed += 1;

    let mut seed_bytes = [0u8; 32];
    seed_bytes[0..16].copy_from_slice(&seed.to_be_bytes());
    let private_key = H256::from_slice(&seed_bytes);
    // The above lines were modified from this original code:
    //let private_key = H256::from_slice(&[(seed as u8 + 1); 32]); //H256::from_low_u64_be((i + 1) as u64);

    let secret_key = libsecp256k1::SecretKey::parse_slice(&private_key[..]).unwrap();
    let public_key = &libsecp256k1::PublicKey::from_secret_key(&secret_key).serialize()[1..65];
    let address = H160::from(H256::from(keccak_256(public_key)));

    let mut data = [0u8; 32];
    data[0..20].copy_from_slice(&address[..]);

    AccountInfo {
        private_key,
        account_id: AccountId32::from(Into::<[u8; 32]>::into(data)),
        address,
    }
}

/// Returns the contract address for the given sender and nonce.
/// Use `U256::as_u64()` to convert the nonce to `u64`.
pub fn contract_address(sender: H160, nonce: u64) -> H160 {
    let mut rlp = RlpStream::new_list(2);
    rlp.append(&sender);
    rlp.append(&nonce);

    H160::from_slice(&keccak_256(&rlp.out())[12..])
}

pub struct LegacyUnsignedTransaction {
    pub nonce: U256,
    pub gas_price: U256,
    pub gas_limit: U256,
    pub action: TransactionAction,
    pub value: U256,
    pub input: Vec<u8>,
}

impl LegacyUnsignedTransaction {
    fn signing_rlp_append(&self, s: &mut RlpStream) {
        s.begin_list(9);
        s.append(&self.nonce);
        s.append(&self.gas_price);
        s.append(&self.gas_limit);
        s.append(&self.action);
        s.append(&self.value);
        s.append(&self.input);
        s.append(&ChainId::get());
        s.append(&0u8);
        s.append(&0u8);
    }

    fn signing_hash(&self) -> H256 {
        let mut stream = RlpStream::new();
        self.signing_rlp_append(&mut stream);
        H256::from(keccak_256(&stream.out()))
    }

    pub fn sign(&self, key: &H256) -> Transaction {
        self.sign_with_chain_id(key, ChainId::get())
    }

    pub fn sign_with_chain_id(&self, key: &H256, chain_id: u64) -> Transaction {
        let hash = self.signing_hash();
        let msg = libsecp256k1::Message::parse(hash.as_fixed_bytes());
        let s = libsecp256k1::sign(
            &msg,
            &libsecp256k1::SecretKey::parse_slice(&key[..]).unwrap(),
        );
        let sig = s.0.serialize();

        let sig = TransactionSignature::new(
            s.1.serialize() as u64 % 2 + chain_id * 2 + 35,
            H256::from_slice(&sig[0..32]),
            H256::from_slice(&sig[32..64]),
        )
        .unwrap();

        Transaction::Legacy(ethereum::LegacyTransaction {
            nonce: self.nonce,
            gas_price: self.gas_price,
            gas_limit: self.gas_limit,
            action: self.action,
            value: self.value,
            input: self.input.clone(),
            signature: sig,
        })
    }
}

pub struct EIP2930UnsignedTransaction {
    pub nonce: U256,
    pub gas_price: U256,
    pub gas_limit: U256,
    pub action: TransactionAction,
    pub value: U256,
    pub input: Vec<u8>,
}

impl EIP2930UnsignedTransaction {
    pub fn sign(&self, secret: &H256, chain_id: Option<u64>) -> Transaction {
        let secret = {
            let mut sk: [u8; 32] = [0u8; 32];
            sk.copy_from_slice(&secret[0..]);
            libsecp256k1::SecretKey::parse(&sk).unwrap()
        };
        let chain_id = chain_id.unwrap_or(ChainId::get());
        let msg = ethereum::EIP2930TransactionMessage {
            chain_id,
            nonce: self.nonce,
            gas_price: self.gas_price,
            gas_limit: self.gas_limit,
            action: self.action,
            value: self.value,
            input: self.input.clone(),
            access_list: vec![],
        };
        let signing_message = libsecp256k1::Message::parse_slice(&msg.hash()[..]).unwrap();

        let (signature, recid) = libsecp256k1::sign(&signing_message, &secret);
        let rs = signature.serialize();
        let r = H256::from_slice(&rs[0..32]);
        let s = H256::from_slice(&rs[32..64]);
        Transaction::EIP2930(ethereum::EIP2930Transaction {
            chain_id: msg.chain_id,
            nonce: msg.nonce,
            gas_price: msg.gas_price,
            gas_limit: msg.gas_limit,
            action: msg.action,
            value: msg.value,
            input: msg.input.clone(),
            access_list: msg.access_list,
            odd_y_parity: recid.serialize() != 0,
            r,
            s,
        })
    }
}

pub struct EIP1559UnsignedTransaction {
    pub nonce: U256,
    pub max_priority_fee_per_gas: U256,
    pub max_fee_per_gas: U256,
    pub gas_limit: U256,
    pub action: TransactionAction,
    pub value: U256,
    pub input: Vec<u8>,
}

impl EIP1559UnsignedTransaction {
    pub fn sign(&self, secret: &H256, chain_id: Option<u64>) -> Transaction {
        let secret = {
            let mut sk: [u8; 32] = [0u8; 32];
            sk.copy_from_slice(&secret[0..]);
            libsecp256k1::SecretKey::parse(&sk).unwrap()
        };
        let chain_id = chain_id.unwrap_or(ChainId::get());
        let msg = ethereum::EIP1559TransactionMessage {
            chain_id,
            nonce: self.nonce,
            max_priority_fee_per_gas: self.max_priority_fee_per_gas,
            max_fee_per_gas: self.max_fee_per_gas,
            gas_limit: self.gas_limit,
            action: self.action,
            value: self.value,
            input: self.input.clone(),
            access_list: vec![],
        };
        let signing_message = libsecp256k1::Message::parse_slice(&msg.hash()[..]).unwrap();

        let (signature, recid) = libsecp256k1::sign(&signing_message, &secret);
        let rs = signature.serialize();
        let r = H256::from_slice(&rs[0..32]);
        let s = H256::from_slice(&rs[32..64]);
        Transaction::EIP1559(ethereum::EIP1559Transaction {
            chain_id: msg.chain_id,
            nonce: msg.nonce,
            max_priority_fee_per_gas: msg.max_priority_fee_per_gas,
            max_fee_per_gas: msg.max_fee_per_gas,
            gas_limit: msg.gas_limit,
            action: msg.action,
            value: msg.value,
            input: msg.input.clone(),
            access_list: msg.access_list,
            odd_y_parity: recid.serialize() != 0,
            r,
            s,
        })
    }
}
