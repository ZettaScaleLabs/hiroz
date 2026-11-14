//! A decoder caps the initial capacity of a sequence by the unread input, whatever count the
//! wire declares.
//!
//! An allocation failure aborts the process, so the test cannot catch it. The cases run in a
//! child process under a lowered address-space limit. See the `hostile` module for the harness
//! and its environment.

// The file runs on 64-bit Linux only. A 32-bit process may reserve the hostile amount, and
// `setrlimit` takes a narrower type there.
#![cfg(all(target_os = "linux", target_pointer_width = "64"))]

use byteorder::LittleEndian;
use hiroz_cdr::{CdrDeserialize, CdrReader};

#[path = "support/hostile.rs"]
mod hostile;

use hostile::{HOSTILE_LENGTH, describe, is_child, limit_address_space, run_child, skipped};

/// One element of 32 bytes on the wire and in memory. 99_999_999 of them need 3.2 GB.
#[derive(Debug)]
struct Big([u64; 4]);

impl CdrDeserialize for Big {
    fn cdr_deserialize<'de, BO: byteorder::ByteOrder>(
        reader: &mut CdrReader<'de, BO>,
    ) -> hiroz_cdr::Result<Self> {
        Ok(Self([
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
            reader.read_u64()?,
        ]))
    }
}

/// A message body without the encapsulation header: a length prefix and nothing after it.
fn length_only(len: u32) -> Vec<u8> {
    len.to_le_bytes().to_vec()
}

/// A valid body for `Vec<Big>`: `n` elements.
fn valid_body(n: u32) -> Vec<u8> {
    let mut v = length_only(n);
    if n > 0 {
        // `u64` elements start on an 8-byte boundary.
        v.extend_from_slice(&[0; 4]);
    }
    for i in 0..n {
        for w in 0..4u64 {
            v.extend_from_slice(&(u64::from(i) * 4 + w).to_le_bytes());
        }
    }
    v
}

/// Generic `Vec<T>::cdr_deserialize` coverage. Generated non-plain sequences use their own loop.
#[test]
fn typed_vec_survives_a_hostile_length() {
    if is_child() {
        limit_address_space();
        let hostile = length_only(HOSTILE_LENGTH);
        let mut r = CdrReader::<LittleEndian>::new(&hostile);
        let err = Vec::<Big>::cdr_deserialize(&mut r).expect_err("hostile length must be an error");
        println!("hostile: {err}");

        // One unread byte: a rule that caps only an empty remainder still reserves the count.
        let mut truncated = length_only(HOSTILE_LENGTH);
        truncated.push(0);
        let mut r = CdrReader::<LittleEndian>::new(&truncated);
        let err = Vec::<Big>::cdr_deserialize(&mut r).expect_err("truncated must be an error");
        println!("truncated: {err}");

        // The same process decodes the next, valid message.
        let valid = valid_body(3);
        let mut r = CdrReader::<LittleEndian>::new(&valid);
        let v = Vec::<Big>::cdr_deserialize(&mut r).expect("valid message must decode");
        assert_eq!(v.len(), 3);
        assert_eq!(v[2].0, [8, 9, 10, 11], "decoded values");
        println!("next valid: {} elements", v.len());
        return;
    }
    let name = "typed_vec_survives_a_hostile_length";
    let out = run_child(name);
    if skipped(&out) {
        return;
    }
    assert!(
        out.status.success(),
        "{name}: the child must survive a hostile length\n{}",
        describe(&out)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("hostile: "), "{name}: {}", describe(&out));
    assert!(stdout.contains("truncated: "), "{name}: {}", describe(&out));
    assert!(
        stdout.contains("next valid: 3 elements"),
        "{name}: {}",
        describe(&out)
    );
}

/// A long sequence that fits its payload still decodes.
#[test]
fn long_valid_sequence_still_decodes() {
    let n = 1000u32;
    let body = valid_body(n);

    let expected: Vec<[u64; 4]> = (0..u64::from(n))
        .map(|i| [i * 4, i * 4 + 1, i * 4 + 2, i * 4 + 3])
        .collect();

    let mut r = CdrReader::<LittleEndian>::new(&body);
    let v = Vec::<Big>::cdr_deserialize(&mut r).expect("typed decode");
    assert_eq!(v.iter().map(|b| b.0).collect::<Vec<_>>(), expected);
}
