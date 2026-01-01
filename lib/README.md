# Key Generation CLI

## Overview

The `key_gen` binary provides a deterministic wallet key generation tool using BIP39 mnemonic phrases. This ensures that keys can be recovered from the mnemonic phrase, making wallet management more secure and user-friendly.

## Usage

Run the key generator:

```bash
cargo run --bin key_gen
```

The tool will:

1. **Generate a BIP39 mnemonic phrase** (12 words)
2. **Display the mnemonic phrase** - **IMPORTANT: Save this securely!**
3. **Derive a private key** from the mnemonic deterministically
4. **Display the public address** (hex-encoded)
5. **Prompt for a directory** to save the keys (default: `./wallet`)
6. **Prompt for a name** for the key pair (default: `default`)
7. **Save the keys** to the specified location

## Example Session

```
=== Deterministic Wallet Key Generator ===

Generated mnemonic phrase:
abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about

⚠️  IMPORTANT: Save this mnemonic phrase in a secure location!
   You will need it to recover your keys.

Public Address (hex): 02a1b2c3d4e5f6...

Enter directory path to save keys (default: ./wallet): 
Enter a name for this key pair (default: default): mywallet

✓ Keys saved successfully!
  Private key: "./wallet/mywallet.priv.cbor"
  Public key: "./wallet/mywallet.pub.pem"

Mnemonic phrase: abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about
Public Address: 02a1b2c3d4e5f6...
```

## Key Files

- **Private Key**: Saved as `{name}.priv.cbor` (CBOR format)
- **Public Key**: Saved as `{name}.pub.pem` (PEM format)

## Recovering Keys from Mnemonic

To recover a key from a mnemonic phrase, you can use the `PrivateKey::from_mnemonic()` method in your code:

```rust
use btclib::crypto::PrivateKey;

let mnemonic = "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about";
let private_key = PrivateKey::from_mnemonic(mnemonic)?;
let public_key = private_key.public_key();
```

## Public Address Format

The public address uses **Bitcoin-style encoding** (Base58Check), which produces shorter, more user-friendly addresses (typically 25-35 characters). The address generation follows the standard Bitcoin algorithm:

1. **SHA256 hash** of the compressed public key
2. **RIPEMD160 hash** of the SHA256 result (20 bytes)
3. **Version byte** (0x00) prepended
4. **Checksum** (first 4 bytes of double SHA256)
5. **Base58Check encoding** of the final result

This format is compatible with Bitcoin address standards and provides better error detection through the checksum.

## Deterministic Wallet Details

### BIP39 Standard

The wallet uses the BIP39 standard for mnemonic phrase generation:
- **12-word mnemonic** (128 bits of entropy)
- **English wordlist**
- Standard PBKDF2 seed derivation

### Key Derivation

1. Mnemonic phrase → Seed (via PBKDF2)
2. Seed → SHA256 hash
3. Hash → Private key (secp256k1)

This ensures that:
- The same mnemonic always produces the same private key
- Keys can be recovered from the mnemonic phrase
- The process is deterministic and reproducible

## Security Best Practices

1. **Never share your mnemonic phrase** - Anyone with access to it can recover your keys
2. **Store the mnemonic securely** - Consider using a password manager or hardware wallet
3. **Backup your mnemonic** - Store it in multiple secure locations
4. **Never commit keys or mnemonics to version control**

## Related Tools

- `block_gen`: Generate blocks (now requires a key file)
- `tx_gen`: Generate transactions (now requires a key file)

Both tools now require you to provide a private key file path, ensuring deterministic key usage.

