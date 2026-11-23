use super::*;

const HEADER: &str = "eyJhbGciOiJIUzI1NiJ9"; // {"alg":"HS256"}
const PAYLOAD: &str = "eyJzdWIiOiJmaXh0dXJlIn0"; // {"sub":"fixture"}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Byte-level fixed point: percent-decode every escape in a pass, then NFKC each valid
/// UTF-8 run (invalid bytes are kept), until nothing changes. Lossy only at the end.
fn fixed_point_bytes(text: &str) -> Vec<u8> {
    let mut current = text.as_bytes().to_vec();
    loop {
        let decoded: Vec<u8> = percent_encoding::percent_decode(&current).collect();
        let mut folded = Vec::new();
        for chunk in decoded.utf8_chunks() {
            let valid: String = chunk.valid().nfkc().collect::<String>().replace('。', ".");
            folded.extend_from_slice(valid.as_bytes());
            folded.extend_from_slice(chunk.invalid());
        }
        if folded == current {
            return current;
        }
        current = folded;
    }
}

fn fixed_point(text: &str) -> String {
    String::from_utf8_lossy(&fixed_point_bytes(text)).into_owned()
}

fn pct(ch: char, rng: &mut Rng) -> String {
    let mut buf = [0; 4];
    ch.encode_utf8(&mut buf)
        .bytes()
        .map(|b| {
            if rng.below(2) == 0 {
                format!("%{b:02X}")
            } else {
                format!("%{b:02x}")
            }
        })
        .collect()
}

fn fullwidth(ch: char) -> char {
    if ('!'..='~').contains(&ch) {
        char::from_u32(ch as u32 - 0x21 + 0xFF01).unwrap()
    } else {
        ch
    }
}

/// One random encoding layer over every char.
fn layer(text: &str, rng: &mut Rng) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        match rng.below(6) {
            0 | 1 => out.push_str(&pct(ch, rng)),
            2 => out.push(fullwidth(ch)),
            3 if ch == '.' => out.push(['。', '．', '｡', '\u{2024}'][rng.below(4) as usize]),
            _ => out.push(ch),
        }
    }
    out
}

#[test]
fn layered_delimiters_never_reach_suggestions() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for seed in 0..512 {
        let depth = 1 + seed % 5;
        for dot in [".", " . ", "\t.\u{2003}"] {
            let token = format!("{HEADER}{dot}{PAYLOAD}{dot}c2ln");
            let mut text = token.clone();
            for _ in 0..depth {
                text = layer(&text, &mut rng);
            }
            assert_eq!(
                fixed_point(&text),
                token.nfkc().collect::<String>(),
                "encoding seed {seed}"
            );
            for input in [
                text.clone(),
                format!("note {text} please"),
                format!("x{text}y"),
            ] {
                assert!(
                    SearchEngine::Google.suggestion_url(&input).is_none(),
                    "leaked synthetic token, seed {seed}"
                );
            }
        }
    }
}
