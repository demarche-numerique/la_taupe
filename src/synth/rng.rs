//! Générateur pseudo-aléatoire déterministe.
//!
//! Volontairement maison plutôt que `rand` : le corpus synthétique doit être
//! reproductible à l'identique d'une version à l'autre, ce qu'un générateur dont
//! l'implémentation peut changer ne garantit pas.

/// xorshift64*, suffisant pour tirer des variantes de documents.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        // un état nul resterait bloqué à zéro
        Rng(seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407)
            | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    /// Entier dans [0, n).
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }

    /// Entier dans [lo, hi].
    pub fn int(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + self.below((hi - lo + 1) as usize) as i64
    }

    /// Flottant dans [lo, hi).
    pub fn float(&mut self, lo: f32, hi: f32) -> f32 {
        let unit = (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32;
        lo + unit * (hi - lo)
    }

    pub fn chance(&mut self, probability: f32) -> bool {
        self.float(0.0, 1.0) < probability
    }

    pub fn pick<'a, T>(&mut self, items: &'a [T]) -> &'a T {
        &items[self.below(items.len())]
    }

    /// Chaîne de `len` chiffres.
    pub fn digits(&mut self, len: usize) -> String {
        (0..len)
            .map(|_| char::from(b'0' + self.below(10) as u8))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_same_sequence() {
        let mut a = Rng::new(42);
        let mut b = Rng::new(42);
        let mut c = Rng::new(43);

        let sa: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let sb: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let sc: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();

        assert_eq!(sa, sb);
        assert_ne!(sa, sc);
    }

    #[test]
    fn seed_zero_is_not_degenerate() {
        let mut rng = Rng::new(0);
        let drawn: Vec<u64> = (0..4).map(|_| rng.next_u64()).collect();

        assert!(drawn.iter().all(|&x| x != 0));
    }

    #[test]
    fn stays_in_bounds() {
        let mut rng = Rng::new(7);

        for _ in 0..500 {
            assert!(rng.below(10) < 10);
            assert!((3..=9).contains(&rng.int(3, 9)));
            assert_eq!(rng.int(5, 5), 5);

            let f = rng.float(-2.0, 2.0);
            assert!((-2.0..2.0).contains(&f));

            assert_eq!(rng.digits(6).len(), 6);
        }
    }
}
