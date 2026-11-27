//! Renderer port 5 (`VICTORY_PORT_CBV=1`, [`super::super::shader::constants_raw`]): constant
//! buffers read as raw bytes.

use super::*;

impl Lower<'_> {
    /// The handle a constant buffer `src` is read through as raw bytes, or
    /// `None` to read it as a constant buffer, as before.
    ///
    /// The console takes constant buffers on 16 bytes, the host on 256: the
    /// layer copied each one the title's tables name into upload memory of
    /// its own at every submission, and each root one not on 256 bytes. A raw
    /// view starts on 16 bytes and a root SRV anywhere on 4, so read so the
    /// card can take the title's bytes where they are:
    /// - a table's (or one picked from the view heap): through the raw twin
    ///   of its slot ([`super::super::shader::raw_views`]), which the host
    ///   makes a raw view of the same bytes; a twin's reads end where the
    ///   view does, as a constant buffer's do;
    /// - a root one: as the root SRV `t<slot>` in
    ///   [`super::super::shader::ROOT_CBV_SPACE`], which the host binds at
    ///   the title's address like any root SRV.
    ///
    /// Anything else (a descriptor built otherwise, or chosen between kinds
    /// of parameter) stays a constant buffer.
    pub(super) fn constants_raw(&mut self, site: Site, src: &Source) -> Result<Option<V>, Why> {
        if !super::super::shader::constants_raw() {
            return Ok(None);
        }
        if let Source::Built { address, .. } = src {
            if self.root_copy.is_some() {
                return Ok(None);
            }
            let Some(&rp) = self.x.native.layout.get(address.param) else {
                return Ok(None);
            };
            let kind = self.x.native.used.iter().find(|u| u.user_data == rp.user_data).map(|u| u.kind);
            if rp.root == RootType::Descriptor && matches!(kind, Some(ResourceType::Cbv) | None) {
                return Ok(Some(self.s.root_srv_handle(rp.slot)));
            }
            return Ok(None);
        }
        if super::super::shader::raw_views() == 0 {
            return Ok(None);
        }
        let x = self.x;
        let scalar = &self.scalar;
        let through_heap = src.leaves().iter().all(|leaf| match leaf {
            Source::Loaded { load, .. } | Source::Fetched { load, .. } => scalar
                .get(load)
                .and_then(|a| x.native.layout.get(a.param))
                .is_some_and(|p| matches!(p.root, RootType::Table | RootType::ResourceHeap)),
            _ => false,
        });
        if !through_heap {
            return Ok(None);
        }
        self.constants_twin(site, src)
    }

    /// The raw twin of the slot a constant buffer is read through, as raw
    /// bytes, when the switch is on and the host makes twins; else `None`.
    pub(super) fn constants_twin(&mut self, site: Site, src: &Source) -> Result<Option<V>, Why> {
        if !super::super::shader::constants_raw() || super::super::shader::raw_views() == 0 {
            return Ok(None);
        }
        // (Raw, so `raw_twin` gives the twin's index.)
        Ok(Some(self.handle(site, src, false, (11, 0))?))
    }
}
