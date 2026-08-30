pub fn mixed(value: u64) -> u64 {
    let mut z = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

pub fn unit(value: u64) -> f32 {
    ((value >> 40) as f32) / 16_777_216.0
}

pub fn in_disc(seed: u64, centre: [f32; 3], radius: f32) -> [f32; 3] {
    if !radius.is_finite() || radius <= 0.0 {
        return centre;
    }
    let noise = mixed(seed);
    let distance = radius * unit(noise).sqrt();
    let angle = unit(mixed(noise)) * std::f32::consts::TAU;
    [
        centre[0] + distance * angle.cos(),
        centre[1] + distance * angle.sin(),
        centre[2],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_drawn_in_a_disc_stays_in_it() {
        let centre = [100.0, -50.0, 7.0];
        for seed in 0..4096 {
            let at = in_disc(seed, centre, 300.0);
            let (dx, dy) = (at[0] - centre[0], at[1] - centre[1]);
            assert!((dx * dx + dy * dy).sqrt() <= 300.0);
            assert_eq!(at[2], centre[2]);
        }
    }

    #[test]
    fn a_disc_with_no_room_gives_back_its_centre() {
        assert_eq!(in_disc(9, [1.0, 2.0, 3.0], 0.0), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn the_draw_spreads_rather_than_repeating_itself() {
        let spots: std::collections::HashSet<u32> = (0..512)
            .map(|seed| in_disc(seed, [0.0; 3], 500.0)[0].to_bits())
            .collect();
        assert!(spots.len() > 500, "the draw collapsed onto {} spots", spots.len());
    }
}
