use crate::key::Bytes;
use crate::Rewind;
use failure::Error;
use leveldb::database::Database;
use leveldb::kv::KV;
use leveldb::options::*;

#[derive(Clone, Deserialize, Serialize, Hash, PartialEq, Eq)]
pub struct UTXOID {
    pub txid: [u8; 32],
    pub vout: u32,
}

pub struct UTXO<'a> {
    address: Option<[u8; 21]>,
    txid: &'a [u8; 32],
    vout: u32,
    value: u64,
}

#[derive(Clone, Deserialize, Serialize)]
pub struct UTXOData {
    address: Option<[u8; 21]>,
    value: u64,
}
impl<'a> From<(&'a UTXOID, UTXOData)> for UTXO<'a> {
    fn from((id, data): (&'a UTXOID, UTXOData)) -> Self {
        UTXO {
            address: data.address,
            txid: &id.txid,
            vout: id.vout,
            value: data.value,
        }
    }
}
impl<'a> From<UTXO<'a>> for (UTXOID, UTXOData) {
    fn from(utxo: UTXO) -> Self {
        (
            UTXOID {
                txid: utxo.txid.clone(),
                vout: utxo.vout,
            },
            UTXOData {
                address: utxo.address,
                value: utxo.value,
            },
        )
    }
}

impl<'a> UTXO<'a> {
    pub fn add(self, db: &Database<Bytes>, raw: Option<(&[u8], u32)>) -> Result<(), Error> {
        let mut utxoid_key = Vec::with_capacity(37);
        utxoid_key.push(5_u8);
        utxoid_key.extend(self.txid);
        if let Some((raw, c)) = raw {
            ldb_try!(db.put(WriteOptions::new(), Bytes::from(&utxoid_key), &c.to_ne_bytes()));
            utxoid_key[0] = 4;
            ldb_try!(db.put(WriteOptions::new(), Bytes::from(&utxoid_key), raw));
        }
        if let Some(address) = self.address {
            let mut addr_key = Vec::with_capacity(26);
            addr_key.push(1_u8);
            addr_key.extend(address.as_ref());
            let len = ldb_try!(db.get(ReadOptions::new(), Bytes::from(&addr_key))).unwrap_or([0_u8; 4].to_vec());
            let mut buf = [0_u8; 4];
            if len.len() == 4 {
                buf.clone_from_slice(&len);
            }
            ldb_try!(db.put(WriteOptions::new(), Bytes::from(&addr_key), &(u32::from_ne_bytes(buf) + 1).to_ne_bytes()));
            addr_key.extend(&len);

            utxoid_key[0] = 2;
            utxoid_key.extend(&self.vout.to_ne_bytes());
            ldb_try!(db.put(WriteOptions::new(), Bytes::from(&utxoid_key), &addr_key));

            let mut addr_value = Vec::with_capacity(44);
            addr_value.extend(self.txid);
            addr_value.extend(&self.vout.to_ne_bytes());
            addr_value.extend(&self.value.to_ne_bytes());
            ldb_try!(db.put(WriteOptions::new(), Bytes::from(&addr_key), &addr_value));
        }
        Ok(())
    }

    pub fn from_txout(txid: &'a [u8; 32], out: &'a bitcoin::TxOut, vout: u32) -> Self {
        UTXO {
            txid,
            vout,
            value: out.value,
            address: {
                if out.script_pubkey.is_p2pkh() {
                    let addr = out
                        .script_pubkey
                        .iter(true)
                        .filter_map(|i| match i {
                            bitcoin::blockdata::script::Instruction::PushBytes(b) => b.get(0..20),
                            _ => None,
                        })
                        .next();
                    let mut buf = [crate::P2PKH; 21];
                    addr.map(|a| {
                        buf[1..].clone_from_slice(a);
                        buf
                    })
                } else if out.script_pubkey.is_p2sh() {
                    let addr = out
                        .script_pubkey
                        .iter(true)
                        .filter_map(|i| match i {
                            bitcoin::blockdata::script::Instruction::PushBytes(b) => b.get(0..20),
                            _ => None,
                        })
                        .next();
                    let mut buf = [crate::P2SH; 21];
                    addr.map(|a| {
                        buf[1..].clone_from_slice(a);
                        buf
                    })
                } else {
                    None
                }
            },
        }
    }

    pub fn from_kv(addr_key: &[u8], addr_value: &[u8]) -> Result<(UTXOID, UTXOData), Error> {
        let mut address = [0_u8; 21];
        address.clone_from_slice(
            &addr_key
                .get(1..22)
                .ok_or(format_err!("unexpected end of input"))?,
        );
        let mut txid = [0_u8; 32];
        txid.clone_from_slice(
            &addr_value
                .get(0..32)
                .ok_or(format_err!("unexpected end of input"))?,
        );
        let mut vout = [0_u8; 4];
        vout.clone_from_slice(
            &addr_value
                .get(32..36)
                .ok_or(format_err!("unexpected end of input"))?,
        );
        let mut value = [0_u8; 8];
        value.clone_from_slice(
            &addr_value
                .get(36..44)
                .ok_or(format_err!("unexpected end of input"))?,
        );
        Ok((
            UTXOID {
                txid,
                vout: u32::from_ne_bytes(vout),
            },
            UTXOData {
                address: Some(address),
                value: u64::from_ne_bytes(value),
            },
        ))
    }
}

impl UTXOID {
    pub fn rem(self, db: &Database<Bytes>, idx: u32, rewind: &mut Rewind) -> Result<(), Error> {
        let mut utxoid_key = Vec::with_capacity(37);
        utxoid_key.push(4_u8);
        utxoid_key.extend(&self.txid);
        let raw = ldb_try!(db.get(ReadOptions::new(), Bytes::from(&utxoid_key)));
        utxoid_key[0] = 5;
        let unspents = ldb_try!(db.get(ReadOptions::new(), Bytes::from(&utxoid_key)))
            .map(|c| {
                let mut buf = [0_u8; 4];
                buf.copy_from_slice(&c);
                u32::from_ne_bytes(buf)
            })
            .unwrap_or(0)
            - 1;
        if unspents == 0 {
            ldb_try!(db.delete(WriteOptions::new(), Bytes::from(&utxoid_key)));
        }
        ldb_try!(db.put(WriteOptions::new(), Bytes::from(&utxoid_key), &unspents.to_ne_bytes()));
        utxoid_key[0] = 2;
        utxoid_key.extend(&self.vout.to_ne_bytes());
        let addr_key = match ldb_try!(db.get(ReadOptions::new(), Bytes::from(&utxoid_key))) {
            Some(a) => a,
            None => return Ok(()),
        };
        let len = ldb_try!(db.get(ReadOptions::new(), Bytes::from(&addr_key[0..22]))).ok_or(format_err!("missing addr length"))?;
        let mut buf = [0_u8; 4];
        if len.len() == 4 {
            buf.clone_from_slice(&len);
        } else {
            bail!("invalid addr length")
        }
        let replacement_idx = u32::from_ne_bytes(buf) - 1;
        let mut replacement_addr_key = Vec::with_capacity(26);
        replacement_addr_key.extend(&addr_key[0..22]);
        replacement_addr_key.extend(&replacement_idx.to_ne_bytes());

        let kv = match ldb_try!(db.get(ReadOptions::new(), Bytes::from(&addr_key))) {
            Some(addr_val) => {
                let a = UTXO::from_kv(&addr_key, &addr_val)?;
                (a.0, Some(a.1))
            }
            None => (self, None),
        };
        rewind[idx as usize % crate::CONFIRMATIONS].insert(kv.0, (kv.1, raw));
        if &replacement_idx.to_ne_bytes() != &addr_key[22..] {
            let replacement_addr_value = ldb_try!(db.get(ReadOptions::new(), Bytes::from(&replacement_addr_key)));
            if let Some(replacement_addr_value) = replacement_addr_value {
                let update_index = UTXO::from_kv(&replacement_addr_key, &replacement_addr_value)?;
                let mut replacement_utxoid_key = Vec::with_capacity(37);
                replacement_utxoid_key.push(2_u8);
                replacement_utxoid_key.extend(&update_index.0.txid);
                replacement_utxoid_key.extend(&update_index.0.vout.to_ne_bytes());
                ldb_try!(db.put(WriteOptions::new(), Bytes::from(&replacement_utxoid_key), &addr_key));
                ldb_try!(db.put(WriteOptions::new(), Bytes::from(&addr_key), &replacement_addr_value));
            }
        }
        ldb_try!(db.delete(WriteOptions::new(), Bytes::from(&replacement_addr_key)));
        ldb_try!(db.delete(WriteOptions::new(), Bytes::from(&utxoid_key)));
        ldb_try!(db.put(WriteOptions::new(), Bytes::from(&addr_key[0..22]), &replacement_idx.to_ne_bytes()));

        Ok(())
    }
}
impl<'a> From<&'a bitcoin::TxIn> for UTXOID {
    fn from(txin: &'a bitcoin::TxIn) -> Self {
        UTXOID {
            txid: {
                let mut buf = [0u8; 32];
                buf.clone_from_slice(&txin.previous_output.txid[..]);
                buf.reverse();
                buf
            },
            vout: txin.previous_output.vout,
        }
    }
}
