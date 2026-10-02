//! Password system (Q10, `password_validate` $A8BA): a plain literal lookup
//! over the 24-row table at $A927 (extracted as `PASSWORDS` in
//! `data/cave_params.rs`). Row = 6 digits | difficulty byte | world<<4 byte.
//! No checksum — any 6-digit string either matches a row or doesn't; failure
//! leaves progress untouched (the original then starts the default game
//! without saying so).

use boulder_dash::data::cave_params::PASSWORDS;

/// What a password encodes: quest (difficulty 0-3) and world (0-5).
/// Passwords always start at the world's first town ($75 low nibble = 0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PasswordTarget {
    pub quest: usize,
    pub world: usize,
}

/// Look up 6 digits in the ROM table. `None` = invalid.
pub fn validate(digits: &[u8; 6]) -> Option<PasswordTarget> {
    PASSWORDS.iter().find(|row| row[..6] == digits[..]).map(|row| {
        PasswordTarget {
            quest: (row[6] & 3) as usize,
            world: (row[7] >> 4) as usize,
        }
    })
}

/// Password matching a progress point, for the game-over screen.
/// Falls back to `000000` (quest 1, world 1) if somehow unmatched.
pub fn for_progress(quest: usize, world: usize) -> [u8; 6] {
    PASSWORDS
        .iter()
        .find(|row| row[6] as usize & 3 == quest && (row[7] >> 4) as usize == world)
        .map(|row| row[..6].try_into().unwrap())
        .unwrap_or([0; 6])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_passwords() {
        assert_eq!(
            validate(&[0, 0, 0, 0, 0, 0]),
            Some(PasswordTarget { quest: 0, world: 0 })
        );
        assert_eq!(
            validate(&[6, 3, 5, 8, 7, 0]),
            Some(PasswordTarget { quest: 0, world: 1 })
        );
        assert_eq!(
            validate(&[1, 8, 4, 9, 0, 4]),
            Some(PasswordTarget { quest: 3, world: 5 })
        );
        assert_eq!(
            validate(&[5, 3, 2, 3, 7, 5]),
            Some(PasswordTarget { quest: 2, world: 0 })
        );
    }

    #[test]
    fn invalid_passwords() {
        assert_eq!(validate(&[1, 1, 1, 1, 1, 1]), None);
        assert_eq!(validate(&[9, 9, 9, 9, 9, 9]), None);
    }

    #[test]
    fn progress_roundtrip() {
        for quest in 0..4 {
            for world in 0..6 {
                let digits = for_progress(quest, world);
                assert_eq!(validate(&digits), Some(PasswordTarget { quest, world }));
            }
        }
    }
}
