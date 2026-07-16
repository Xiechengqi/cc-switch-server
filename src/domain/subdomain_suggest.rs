use rand::Rng;

pub const MIN_SUBDOMAIN_LEN: usize = 6;
pub const MAX_SUBDOMAIN_LEN: usize = 30;
pub const MAX_WORDS_PER_CANDIDATE: usize = 8;
pub const COLLISION_SUFFIX_LEN: usize = 2;
pub const SUGGEST_MAX_ATTEMPTS: usize = 8;
pub const SUGGEST_COLLISION_BREAKER_AFTER: usize = 6;

pub const RESERVED_SUBDOMAINS: &[&str] = &["admin", "api", "www", "router", "cdn-cgi"];

const WORDLIST: &str = include_str!("../../assets/wordlists/client-subdomain-words.txt");

pub fn wordlist() -> Vec<&'static str> {
    WORDLIST
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && line.chars().all(|ch| ch.is_ascii_lowercase()))
        .collect()
}

pub fn is_reserved_subdomain(value: &str) -> bool {
    RESERVED_SUBDOMAINS.contains(&value)
}

pub fn generate_memorable_subdomain(rng: &mut impl Rng) -> String {
    let words = wordlist();
    debug_assert!(
        !words.is_empty(),
        "client subdomain wordlist must not be empty"
    );

    let mut result = String::new();
    let mut count = 0usize;
    while result.len() < MIN_SUBDOMAIN_LEN && count < MAX_WORDS_PER_CANDIDATE {
        let word = words[rng.gen_range(0..words.len())];
        result.push_str(word);
        count += 1;
    }
    while result.len() < MIN_SUBDOMAIN_LEN {
        result.push((b'a' + rng.gen_range(0..26)) as char);
    }
    if result.len() > MAX_SUBDOMAIN_LEN {
        result.truncate(MAX_SUBDOMAIN_LEN);
    }
    result
}

pub fn generate_client_subdomain(rng: &mut impl Rng) -> String {
    generate_memorable_subdomain(rng)
}

pub fn generate_share_slug(rng: &mut impl Rng) -> String {
    generate_memorable_subdomain(rng)
}

pub fn append_random_letters(rng: &mut impl Rng, base: &str, count: usize) -> String {
    let mut result = base.to_string();
    for _ in 0..count {
        result.push((b'a' + rng.gen_range(0..26)) as char);
    }
    if result.len() > MAX_SUBDOMAIN_LEN {
        result.truncate(MAX_SUBDOMAIN_LEN);
    }
    result
}

pub fn generate_candidate(rng: &mut impl Rng, attempt: usize) -> String {
    let base = generate_memorable_subdomain(rng);
    if attempt >= SUGGEST_COLLISION_BREAKER_AFTER {
        append_random_letters(rng, &base, COLLISION_SUFFIX_LEN)
    } else {
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    #[test]
    fn wordlist_is_non_empty_and_valid() {
        let words = wordlist();
        assert!(words.len() >= 100);
        for word in words {
            assert!(!word.is_empty());
            assert!(word.chars().all(|ch| ch.is_ascii_lowercase()));
            assert!(!is_reserved_subdomain(word));
        }
    }

    #[test]
    fn generate_memorable_subdomain_meets_minimum_length() {
        let mut rng = StdRng::seed_from_u64(42);
        for _ in 0..200 {
            let value = generate_memorable_subdomain(&mut rng);
            assert!(value.len() >= MIN_SUBDOMAIN_LEN);
            assert!(value.len() <= MAX_SUBDOMAIN_LEN);
            assert!(value.chars().all(|ch| ch.is_ascii_lowercase()));
        }
    }

    #[test]
    fn client_and_share_generators_emit_valid_public_slugs() {
        let mut rng = StdRng::seed_from_u64(20260716);
        for _ in 0..200 {
            let client = generate_client_subdomain(&mut rng);
            let share = generate_share_slug(&mut rng);
            assert!(crate::domain::router::ClientSubdomain::parse(&client).is_ok());
            assert!(crate::domain::router::ShareSlug::parse(&share).is_ok());
        }
    }

    #[test]
    fn short_words_are_appended_until_minimum_length() {
        let mut rng = StdRng::seed_from_u64(7);
        let short_words = ["go", "at", "ox", "up"];
        let mut result = String::new();
        let mut count = 0usize;
        while result.len() < MIN_SUBDOMAIN_LEN && count < MAX_WORDS_PER_CANDIDATE {
            let word = short_words[rng.gen_range(0..short_words.len())];
            result.push_str(word);
            count += 1;
        }
        assert!(result.len() >= MIN_SUBDOMAIN_LEN);
    }

    #[test]
    fn collision_breaker_extends_candidate() {
        let mut rng = StdRng::seed_from_u64(99);
        let base = "maple";
        let extended = append_random_letters(&mut rng, base, COLLISION_SUFFIX_LEN);
        assert_eq!(extended.len(), base.len() + COLLISION_SUFFIX_LEN);
    }

    #[test]
    fn generate_candidate_adds_suffix_after_threshold() {
        let mut rng = StdRng::seed_from_u64(123);
        let early = generate_candidate(&mut rng, 0);
        let late = generate_candidate(&mut rng, SUGGEST_COLLISION_BREAKER_AFTER);
        assert!(early.len() >= MIN_SUBDOMAIN_LEN);
        assert!(late.len() >= MIN_SUBDOMAIN_LEN + COLLISION_SUFFIX_LEN);
    }
}
