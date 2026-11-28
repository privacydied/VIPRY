//! The padding words of the title's placed view descriptions, zeroed.
//!
//! The title's descriptions are structs on its stack, and their padding holds
//! whatever was there. Each value made a new shape of a placed view, and the
//! same texture another key of the maps of placed textures: one fork's run
//! kept 1.9 million shapes after eight minutes, 5,000 new a second standing
//! still, word 1 of the resource taking 12,648 values (the top of a host
//! pointer and -1.0 among them). After advantyx/VIctyx `descriptor.rs`
//! (`unpadded_resource`, `unpadded_view`; GPL-3.0, as this).

/// A resource description's words with its two padding words (after
/// `Dimension`, and at the end) zeroed.
pub(crate) fn resource(mut resource: [u32; 14]) -> [u32; 14] {
    resource[1] = 0;
    resource[13] = 0;
    resource
}

/// A placed view description's words with the padding of its buffer forms
/// zeroed: a shader resource view's word 3 (before its union, which holds a
/// 64-bit field) and, for a buffer view, word 9 (the union's tail after the
/// flags). Other forms use those words (a texture array view ends at word 9).
pub(crate) fn view(unordered: bool, mut desc: [u32; 16]) -> [u32; 16] {
    /// `D3D12_SRV_DIMENSION_BUFFER` and `D3D12_UAV_DIMENSION_BUFFER`.
    const BUFFER: u32 = 1;
    if !unordered {
        desc[3] = 0;
    }
    if desc[1] == BUFFER {
        desc[9] = 0;
    }
    desc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer(pad: u32) -> [u32; 16] {
        let mut d = [0u32; 16];
        d[1] = 1;
        d[3] = pad;
        d[9] = pad;
        d
    }

    #[test]
    fn padding_makes_no_new_shape() {
        let r = |pad: u32| {
            let mut r: [u32; 14] = std::array::from_fn(|k| 100 + k as u32);
            (r[1], r[13]) = (pad, pad);
            r
        };
        assert_eq!(resource(r(7)), resource(r(0x7ffe)));
        assert_eq!(resource(r(7))[1], 0);
        assert_eq!(view(false, buffer(7)), view(false, buffer(0xbf80_0000)));
        assert_eq!(view(true, buffer(7))[9], 0);
        assert_eq!(
            view(true, buffer(7))[3],
            7,
            "an unordered view's word 3 is a field"
        );
    }

    #[test]
    fn a_texture_views_words_are_kept() {
        let mut d: [u32; 16] = std::array::from_fn(|k| k as u32 + 10);
        d[1] = 4; // a 2D texture
        assert_eq!(view(true, d), d);
        assert_eq!(view(false, d)[9], d[9]);
    }
}
