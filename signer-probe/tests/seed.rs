use bitcoin::{secp256k1::Secp256k1, Network};
use signer_probe::seed::SeedSession;

#[test]
fn recovery_matches_independent_embit_vectors() {
    let secp = Secp256k1::new();
    for row in include_str!("seed-vectors.tsv").lines() {
        let f: Vec<_> = row.split('\t').collect();
        assert_eq!(f.len(), 6);
        let seed = SeedSession::recover(f[0], f[1], Network::Testnet).unwrap();
        assert_eq!(seed.fingerprint(&secp).unwrap().to_string(), f[2]);
        assert_eq!(seed.account_xpub(&secp).unwrap().to_string(), f[3]);
        assert_eq!(seed.receive_address(&secp, 0).unwrap().to_string(), f[4]);
        assert_eq!(
            seed.receive_address(&secp, 1000).unwrap().to_string(),
            f[5]
        );
        for (change, branch) in [(false, 0), (true, 1)] {
            assert_eq!(
                seed.descriptor(&secp, change).unwrap(),
                format!("wpkh([{}/84h/1h/0h]{}/{branch}/*)", f[2], f[3])
            );
        }
    }
}

#[test]
fn test_networks_share_keys_but_regtest_uses_its_address_prefix() {
    let secp = Secp256k1::new();
    let row: Vec<_> = include_str!("seed-vectors.tsv")
        .lines()
        .next()
        .unwrap()
        .split('\t')
        .collect();
    for network in [Network::Signet, Network::Regtest] {
        let seed = SeedSession::recover(row[0], row[1], network).unwrap();
        assert_eq!(seed.account_xpub(&secp).unwrap().to_string(), row[3]);
        let address = seed.receive_address(&secp, 0).unwrap();
        let test = SeedSession::recover(row[0], row[1], Network::Testnet)
            .unwrap()
            .receive_address(&secp, 0)
            .unwrap();
        assert_eq!(address.script_pubkey(), test.script_pubkey());
        assert!(address
            .to_string()
            .starts_with(if network == Network::Regtest {
                "bcrt1"
            } else {
                "tb1"
            }));
    }
}
