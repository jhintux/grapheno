use crate::sha256::Hash;
use crate::util::Saveable;
use ecdsa::{
    Signature as ECDSASignature, SigningKey, VerifyingKey,
    signature::{Signer, Verifier},
};
use k256::{Secp256k1, pkcs8::EncodePublicKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use std::io::{Error as IoError, ErrorKind as IoErrorKind, Read, Result as IoResult, Write};
use std::fmt;
use bip39::{Mnemonic, Language};
use sha2::{Sha256, Digest};
use ripemd::Ripemd160;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Signature(ECDSASignature<Secp256k1>);

impl Signature {
    // sign a crate::types::TransactionOutput from its Sha256 hash
    pub fn sign_output(output_hash: &Hash, private_key: &PrivateKey) -> Self {
        let signing_key = &private_key.0;
        let signature = signing_key.sign(&output_hash.as_bytes());
        Signature(signature)
    }

    // verify a signature
    pub fn verify(&self, output_hash: &Hash, public_key: &PublicKey) -> bool {
        public_key
            .0
            .verify(&output_hash.as_bytes(), &self.0)
            .is_ok()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Ord, PartialOrd)]
pub struct PublicKey(VerifyingKey<Secp256k1>);

impl PublicKey {
    /// Get the public key as hex-encoded bytes
    pub fn to_hex(&self) -> String {
        hex::encode(self.0.to_encoded_point(false).as_bytes())
    }

    /// Generate a Bitcoin-style address from the public key
    /// Algorithm:
    /// 1. SHA256 hash of compressed public key
    /// 2. RIPEMD160 hash of the SHA256 result (20 bytes)
    /// 3. Add version byte (0x00 for mainnet)
    /// 4. Double SHA256 of (version + hash), take first 4 bytes as checksum
    /// 5. Base58 encode (version + hash + checksum)
    pub fn to_address(&self) -> String {
        // Step 1: Get compressed public key bytes
        let encoded_point = self.0.to_encoded_point(true);
        let pub_key_bytes = encoded_point.as_bytes();
        
        // Step 2: SHA256 hash
        let mut hasher = Sha256::new();
        hasher.update(pub_key_bytes);
        let sha256_hash = hasher.finalize();
        
        // Step 3: RIPEMD160 hash (20 bytes)
        let mut ripemd_hasher = Ripemd160::new();
        ripemd_hasher.update(&sha256_hash);
        let pub_key_hash = ripemd_hasher.finalize();
        
        // Step 4: Add version byte (0x00 for mainnet-style addresses)
        let version: u8 = 0x00;
        let mut versioned_hash = vec![version];
        versioned_hash.extend_from_slice(&pub_key_hash);
        
        // Step 5: Calculate checksum (first 4 bytes of double SHA256)
        let mut checksum_hasher = Sha256::new();
        checksum_hasher.update(&versioned_hash);
        let first_hash = checksum_hasher.finalize();
        
        let mut checksum_hasher2 = Sha256::new();
        checksum_hasher2.update(&first_hash);
        let second_hash = checksum_hasher2.finalize();
        
        let checksum = &second_hash[..4];
        
        // Step 6: Combine version + hash + checksum and Base58 encode
        let mut address_bytes = versioned_hash;
        address_bytes.extend_from_slice(checksum);
        
        bs58::encode(&address_bytes).into_string()
    }

    /// Validate a Bitcoin-style address format
    /// Returns true if the address is valid Base58Check format
    pub fn validate_address(address: &str) -> Result<bool, String> {
        // Decode Base58
        let decoded = bs58::decode(address)
            .into_vec()
            .map_err(|e| format!("Invalid Base58 encoding: {}", e))?;

        // Address should be at least 25 bytes (version + hash + checksum)
        if decoded.len() < 25 {
            return Ok(false);
        }

        // Split into version+hash and checksum
        let version_and_hash = &decoded[..decoded.len() - 4];
        let provided_checksum = &decoded[decoded.len() - 4..];

        // Calculate expected checksum (double SHA256)
        let mut hasher = Sha256::new();
        hasher.update(version_and_hash);
        let first_hash = hasher.finalize();

        let mut hasher2 = Sha256::new();
        hasher2.update(&first_hash);
        let second_hash = hasher2.finalize();

        let expected_checksum = &second_hash[..4];

        // Verify checksum matches
        if provided_checksum != expected_checksum {
            return Ok(false);
        }

        // Verify version byte is 0x00 (mainnet-style)
        if decoded[0] != 0x00 {
            return Ok(false);
        }

        Ok(true)
    }
}

impl fmt::Display for PublicKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_address())
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct PrivateKey(#[serde(with = "signkey_serde")] SigningKey<Secp256k1>);

impl PrivateKey {
    /// Generate a new random private key (non-deterministic)
    /// Deprecated: Use from_mnemonic() for deterministic key generation
    pub fn new_key() -> Self {
        PrivateKey(SigningKey::random(&mut OsRng))
    }

    /// Generate a private key from a BIP39 mnemonic phrase
    pub fn from_mnemonic(mnemonic: &str) -> Result<Self, String> {
        let mnemonic = Mnemonic::parse_in_normalized(Language::English, mnemonic)
            .map_err(|e| format!("Invalid mnemonic: {}", e))?;
        let seed = mnemonic.to_seed("");
        Self::from_seed(&seed)
    }

    /// Generate a private key from a seed (64 bytes)
    pub fn from_seed(seed: &[u8]) -> Result<Self, String> {
        // Use SHA256 of the seed to derive the private key deterministically
        use sha256::digest;
        let seed_hash = digest(seed);
        let seed_bytes = hex::decode(seed_hash)
            .map_err(|e| format!("Failed to decode hash: {}", e))?;
        
        // Take first 32 bytes for the private key
        let key_bytes: [u8; 32] = seed_bytes[..32]
            .try_into()
            .map_err(|_| "Failed to convert to 32-byte array")?;
        
        // Ensure the key is valid for secp256k1 (must be < curve order)
        // k256::SigningKey handles this validation
        let signing_key = SigningKey::from_slice(&key_bytes)
            .map_err(|e| format!("Failed to create signing key from seed: {}", e))?;
        
        Ok(PrivateKey(signing_key))
    }

    pub fn public_key(&self) -> PublicKey {
        PublicKey(self.0.verifying_key().clone())
    }
}

mod signkey_serde {
    use serde::Deserialize;
    pub fn serialize<S>(
        key: &super::SigningKey<super::Secp256k1>,
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_bytes(&key.to_bytes())
    }
    pub fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<super::SigningKey<super::Secp256k1>, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bytes: Vec<u8> = Vec::<u8>::deserialize(deserializer)?;
        Ok(super::SigningKey::from_slice(&bytes).unwrap())
    }
}

impl Saveable for PrivateKey {
    fn load<I: Read>(reader: I) -> IoResult<Self> {
        ciborium::de::from_reader(reader)
            .map_err(|_| IoError::new(IoErrorKind::InvalidData, "Failed to deserialize PrivateKey"))
    }

    fn save<O: Write>(&self, writer: O) -> IoResult<()> {
        ciborium::ser::into_writer(self, writer).map_err(|_| {
            IoError::new(IoErrorKind::InvalidData, "Failed to serialize PrivateKey")
        })?;
        Ok(())
    }
}

// save and load as PEM
impl Saveable for PublicKey {
    fn load<I: Read>(mut reader: I) -> IoResult<Self> {
        // read PEM-encoded public key into string
        let mut buf = String::new();
        reader.read_to_string(&mut buf)?;
        // decode the public key from PEM
        let public_key = buf
            .parse()
            .map_err(|_| IoError::new(IoErrorKind::InvalidData, "Failed to parse PublicKey"))?;
        Ok(PublicKey(public_key))
    }

    fn save<O: Write>(&self, mut writer: O) -> IoResult<()> {
        let s = self
            .0
            .to_public_key_pem(Default::default())
            .map_err(|_| IoError::new(IoErrorKind::InvalidData, "Failed to serialize PublicKey"))?;
        writer.write_all(s.as_bytes())?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha256::Hash;

    #[test]
    fn test_from_mnemonic_valid() {
        // Test with a valid BIP39 mnemonic (12 words)
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let result = PrivateKey::from_mnemonic(mnemonic);
        assert!(result.is_ok(), "Should successfully create key from valid mnemonic");
        
        let key = result.unwrap();
        let public_key = key.public_key();
        assert!(!public_key.to_hex().is_empty(), "Public key should have hex representation");
    }

    #[test]
    fn test_from_mnemonic_deterministic() {
        // Test that the same mnemonic always produces the same key
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        
        let key1 = PrivateKey::from_mnemonic(mnemonic).unwrap();
        let key2 = PrivateKey::from_mnemonic(mnemonic).unwrap();
        
        let pub1 = key1.public_key();
        let pub2 = key2.public_key();
        
        assert_eq!(pub1.to_address(), pub2.to_address(), "Same mnemonic should produce same address");
    }

    #[test]
    fn test_from_mnemonic_invalid() {
        // Test with invalid mnemonic
        let invalid_mnemonic = "not a valid mnemonic phrase";
        let result = PrivateKey::from_mnemonic(invalid_mnemonic);
        assert!(result.is_err(), "Should fail with invalid mnemonic");
    }

    #[test]
    fn test_from_mnemonic_wrong_word_count() {
        // Test with wrong number of words
        let wrong_count = "abandon abandon abandon";
        let result = PrivateKey::from_mnemonic(wrong_count);
        assert!(result.is_err(), "Should fail with wrong word count");
    }

    #[test]
    fn test_from_seed_valid() {
        // Test with a valid seed
        let seed = b"test seed for key generation";
        let result = PrivateKey::from_seed(seed);
        assert!(result.is_ok(), "Should successfully create key from seed");
        
        let key = result.unwrap();
        let public_key = key.public_key();
        assert!(!public_key.to_hex().is_empty(), "Public key should have hex representation");
    }

    #[test]
    fn test_from_seed_deterministic() {
        // Test that the same seed always produces the same key
        let seed = b"deterministic test seed";
        
        let key1 = PrivateKey::from_seed(seed).unwrap();
        let key2 = PrivateKey::from_seed(seed).unwrap();
        
        let pub1 = key1.public_key();
        let pub2 = key2.public_key();
        
        assert_eq!(pub1.to_address(), pub2.to_address(), "Same seed should produce same address");
    }

    #[test]
    fn test_from_seed_different_seeds() {
        // Test that different seeds produce different keys
        let seed1 = b"seed one";
        let seed2 = b"seed two";
        
        let key1 = PrivateKey::from_seed(seed1).unwrap();
        let key2 = PrivateKey::from_seed(seed2).unwrap();
        
        let pub1 = key1.public_key();
        let pub2 = key2.public_key();
        
        assert_ne!(pub1.to_address(), pub2.to_address(), "Different seeds should produce different addresses");
    }

    #[test]
    fn test_from_seed_various_lengths() {
        // Test with seeds of various lengths
        let seed1: &[u8] = b"short";
        let seed2: &[u8] = b"medium length seed";
        let seed3: &[u8] = b"this is a much longer seed that should still work fine";
        let seed4: &[u8] = &[0u8; 64]; // 64-byte seed (BIP39 standard)

        let seeds: Vec<&[u8]> = vec![seed1, seed2, seed3, seed4];

        for seed in seeds {
            let result = PrivateKey::from_seed(seed);
            assert!(result.is_ok(), "Should handle seed of length {}", seed.len());
        }
    }

    #[test]
    fn test_public_key_to_hex() {
        // Test that to_hex() produces a valid hex string
        let key = PrivateKey::new_key();
        let public_key = key.public_key();
        let hex = public_key.to_hex();
        
        assert!(!hex.is_empty(), "Hex string should not be empty");
        assert!(hex.len() > 0, "Hex string should have content");
        
        // Verify it's valid hex (only contains 0-9, a-f)
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit()), 
                "Hex string should only contain hex digits");
    }

    #[test]
    fn test_public_key_display() {
        // Test Display implementation (should use Bitcoin-style address)
        let key = PrivateKey::new_key();
        let public_key = key.public_key();
        let address = public_key.to_address();
        let display = format!("{}", public_key);
        
        assert_eq!(address, display, "Display should match to_address()");
        assert!(address.len() >= 25 && address.len() <= 35, 
                "Bitcoin-style address should be 25-35 characters");
    }

    #[test]
    fn test_mnemonic_to_seed_consistency() {
        // Test that mnemonic-based keys are consistent with direct seed usage
        let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic_obj = Mnemonic::parse_in_normalized(Language::English, mnemonic).unwrap();
        let seed = mnemonic_obj.to_seed("");
        
        let key_from_mnemonic = PrivateKey::from_mnemonic(mnemonic).unwrap();
        let key_from_seed = PrivateKey::from_seed(&seed).unwrap();
        
        let pub_from_mnemonic = key_from_mnemonic.public_key();
        let pub_from_seed = key_from_seed.public_key();
        
        assert_eq!(pub_from_mnemonic.to_address(), pub_from_seed.to_address(),
                   "Key from mnemonic should match key from derived seed");
    }

    #[test]
    fn test_public_key_consistency() {
        // Test that public key is consistent across multiple calls
        let key = PrivateKey::new_key();
        let pub1 = key.public_key();
        let pub2 = key.public_key();
        
        assert_eq!(pub1.to_address(), pub2.to_address(), 
                   "Multiple calls to public_key() should return same address");
    }

    #[test]
    fn test_key_generation_produces_valid_keys() {
        // Test that generated keys can be used for signing
        let key = PrivateKey::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let public_key = key.public_key();
        
        // Create a test hash
        let test_data = b"test data";
        let hash = Hash::hash(test_data);
        
        // Sign the hash
        let signature = Signature::sign_output(&hash, &key);
        
        // Verify the signature
        assert!(signature.verify(&hash, &public_key), 
                "Generated key should be able to sign and verify");
    }

    #[test]
    fn test_multiple_mnemonics_produce_different_keys() {
        // Test that different mnemonics produce different keys
        let mnemonic1 = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
        let mnemonic2 = "zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo zoo wrong";
        
        let key1 = PrivateKey::from_mnemonic(mnemonic1).unwrap();
        let key2 = PrivateKey::from_mnemonic(mnemonic2).unwrap();
        
        let pub1 = key1.public_key();
        let pub2 = key2.public_key();
        
        assert_ne!(pub1.to_address(), pub2.to_address(), 
                   "Different mnemonics should produce different addresses");
    }

    #[test]
    fn test_public_key_address_format() {
        // Test that Bitcoin-style address has expected characteristics
        let key = PrivateKey::new_key();
        let public_key = key.public_key();
        let address = public_key.to_address();
        
        // Bitcoin addresses are typically 25-35 characters (Base58 encoded)
        assert!(address.len() >= 25 && address.len() <= 35, 
                "Bitcoin-style address should be 25-35 characters");
        
        // Base58 only contains alphanumeric characters (excluding 0, O, I, l)
        assert!(address.chars().all(|c| c.is_alphanumeric() && 
                c != '0' && c != 'O' && c != 'I' && c != 'l'),
                "Address should only contain Base58 characters");
    }

    #[test]
    fn test_public_key_address_deterministic() {
        // Test that the same public key always produces the same address
        let key = PrivateKey::from_mnemonic(
            "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about"
        ).unwrap();
        let public_key = key.public_key();
        
        let address1 = public_key.to_address();
        let address2 = public_key.to_address();
        
        assert_eq!(address1, address2, "Same public key should produce same address");
    }
}
