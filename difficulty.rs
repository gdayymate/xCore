

```rust
use serde::{Serialize, Deserialize};
use std::cmp::{min, max};

pub const MAX_DIFFICULTY_BITS: u32 = 0x1f000001;
pub const MIN_DIFFICULTY_BITS: u32 = 0x207fffff;
pub const GENESIS_BLOCK_DIFFICULTY: u32 = 0x207fffff;
pub const BLOCK_REWARD: u64 = 50; // 50 XTAL

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Difficulty {
    pub bits: u32,
}

impl Difficulty {
    pub fn new(bits: u32) -> Self {
        Difficulty { bits: max(min(bits, MAX_DIFFICULTY_BITS), MIN_DIFFICULTY_BITS) }
    }

    pub fn target(&self) -> [u8; 16] {
        let exponent = self.bits >> 24;
        let mantissa = self.bits & 0x00ffffff;
        let mut target = [0u8; 16];
       
        let significant_bytes = min(exponent as usize, 16);
        let start_idx = 16 - significant_bytes;
       
        target[start_idx] = (mantissa >> 16) as u8;
        if significant_bytes > 1 {
            target[start_idx + 1] = ((mantissa >> 8) & 0xff) as u8;
        }
        if significant_bytes > 2 {
            target[start_idx + 2] = (mantissa & 0xff) as u8;
        }
       
        target
    }

    pub fn from_target(target: &[u8; 16]) -> Self {
        let mut significant_bytes = 0;
        let mut mantissa = 0u32;
        for (i, &byte) in target.iter().enumerate() {
            if byte != 0 {
                significant_bytes = 16 - i;
                mantissa = (byte as u32) << 16;
                if i + 1 < 16 { mantissa |= (target[i + 1] as u32) << 8; }
                if i + 2 < 16 { mantissa |= target[i + 2] as u32; }
                break;
            }
        }
        let bits = (significant_bytes as u32) << 24 | (mantissa & 0x00ffffff);
        Difficulty::new(bits)
    }

    pub fn to_float(&self) -> f64 {
        let exponent = self.bits >> 24;
        let mantissa = self.bits & 0x00ffffff;
        (mantissa as f64) * 2f64.powi(8 * (exponent as i32 - 3))
    }

    pub fn to_target(&self) -> u128 {
        let exponent = self.bits >> 24;
        let mantissa = self.bits & 0x00ffffff;
        let mut target = 0u128;
        
        target |= (mantissa as u128) << (8 * (exponent as u128 - 3));
        target
    }

    pub fn relative_difficulty(&self, other: &Difficulty) -> f64 {
        other.to_float() / self.to_float()
    }

    pub fn stem_difficulty(&self) -> Self {
        let target = self.to_target();
        let stem_target = target.saturating_mul(2); // Double the target (half the difficulty)
        Difficulty::from_target_u128(stem_target)
    }

    pub fn from_target_u128(target: u128) -> Self {
        let mut bits = 0u32;
        let mut shifted_target = target;

        // Find the most significant byte
        let mut exponent = 32;
        while shifted_target > 0 && exponent > 3 {
            shifted_target >>= 8;
            exponent -= 1;
        }

        // Extract mantissa (3 most significant bytes)
        let mantissa = min(target >> (8 * (exponent - 3)), 0x00ffffff) as u32;

        bits = (exponent << 24) | mantissa;
        Difficulty::new(bits)
    }
}

pub fn adjust_difficulty(current_difficulty: Difficulty, actual_timespan: u64, target_timespan: u64) -> (Difficulty, f64) {
    const MAX_ADJUSTMENT_RATIO: u64 = 4;
    let adjusted_timespan = min(max(actual_timespan, target_timespan / MAX_ADJUSTMENT_RATIO), target_timespan * MAX_ADJUSTMENT_RATIO);
    let current_target = current_difficulty.to_target();
    let new_target = (current_target as u128 * adjusted_timespan as u128 / target_timespan as u128).min(u128::MAX);
    let new_difficulty = Difficulty::from_target_u128(new_target);
    let percent_change = (new_difficulty.bits as f64 - current_difficulty.bits as f64) / current_difficulty.bits as f64 * 100.0;
    (new_difficulty, percent_change)
}
