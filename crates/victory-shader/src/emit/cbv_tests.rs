//! Renderer port 5 (`VICTORY_PORT_CBV=1`): constant buffers read as raw bytes.

/// Renderer port 5 (`VICTORY_PORT_CBV=1`): a table's constant buffer is
/// read as raw bytes through the raw twin of its slot, at a constant and
/// at a computed offset, and gives the dwords the constant buffer read
/// gives without the switch; the slot itself (a constant buffer view, of
/// other bytes here) is not read.
#[test]
fn table_constant_buffers_read_as_raw_bytes_through_their_twins() {
    let mut words: Vec<u32> = vec![
        0xBE85_0380, // s_mov_b32 s5, 0
        0xF408_0202,
        0xFA00_0000, // s_load_dwordx4 s[8:11], s[4:5], 0x0 (the constant buffer)
        0xF408_0302,
        0xFA00_0020, // s_load_dwordx4 s[12:15], s[4:5], 0x20 (the UAV)
        0xF428_0404,
        0xFA00_0010, // s_buffer_load_dwordx4 s[16:19], s[8:11], 0x10
        0xF420_0504,
        0x0000_0000, // s_buffer_load_dword s20, s[8:11], s0
    ];
    for (out, s) in (16..21u32).enumerate() {
        // v_mov_b32 v1, s<s>; buffer_store_dword v1, s[12:15], offset 4 * out
        words.push(0x7E00_0000 | 1 << 17 | 0x001 << 9 | s);
        words.push(0x38 << 26 | 0x1C << 18 | (4 * out as u32));
        words.push(0x80 << 24 | (12 >> 2) << 16 | 1 << 8);
    }
    words.push(0xBF81_0000); // s_endpgm
    let table = |kind, at| crate::UsedResource {
        kind,
        root: crate::RootType::Table,
        table_offset_dwords: at,
        lower_bound: 0,
        stride: 4,
        dwords: 4,
        user_data: 4,
        slot: 1,
    };
    let native = super::NativeProgram {
        stage: super::HwStage::Cs,
        wave64: false,
        vgprs: 4,
        layout: vec![
            crate::RootParam { root: crate::RootType::Constants, table_offset_dwords: 0, sgprs: 4, base_register: 0, user_data: 0, slot: 0 },
            crate::RootParam { root: crate::RootType::Table, table_offset_dwords: 0, sgprs: 1, base_register: 0, user_data: 4, slot: 1 },
        ],
        used: vec![table(crate::ResourceType::Cbv, 0), table(crate::ResourceType::Uav, 8)],
        io: crate::native::StageIo::Cs(crate::native::ComputeIo { rsrc2: 5 << 1, threads: [1, 1, 1] }),
        code: words.iter().flat_map(|w| w.to_le_bytes()).collect(),
    };
    // The twins start at slot 2: the constant buffer's (slot 0) is 2, the UAV's (1) 3.
    const TWINS: u32 = 2;
    let lowered = |raw: bool| {
        super::shader::CONSTANTS_RAW_HERE.set(Some(raw));
        super::shader::RAW_VIEWS_HERE.set(Some(if raw { TWINS } else { 0 }));
        let p = crate::ir::build(&native.code, native.wave64, native.vgprs).unwrap();
        let res = crate::resource::track(&p, &native.layout, native.stage, None).unwrap();
        let io = crate::io::raise(&p, &native).unwrap();
        let st = crate::structure::structure(&p).unwrap();
        let l = super::lower::lower(&super::lower::Parts {
            native: &native,
            p: &p,
            st: &st,
            io: &io,
            res: &res,
            inputs: Vec::new(),
            outputs: Vec::new(),
            prims: Vec::new(),
            mesh_stride: 0,
        })
        .unwrap();
        super::shader::CONSTANTS_RAW_HERE.set(None);
        super::shader::RAW_VIEWS_HERE.set(None);
        l
    };
    let (before, after) = (lowered(false), lowered(true));
    assert_ne!(before.dxil, after.dxil, "the switch changes the translation");
    let data: Vec<u32> = (0..16u32).map(|i| 0x0101_0101u32.wrapping_mul(i + 3) ^ 0x4000_0000).collect();
    let other: Vec<u32> = vec![0xdead_beef; 64];
    let s0 = 8u32;
    let want: Vec<u32> = vec![data[4], data[5], data[6], data[7], data[(s0 / 4) as usize]];
    let _one = super::valu_tests::ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    for (raw, l) in [(false, before), (true, after)] {
        let mut dxil = l.dxil;
        if let Ok(mut v) = super::validator::Validator::load(super::validator::Validator::default_path().as_deref()) {
            v.validate(&mut dxil).unwrap();
        }
        let Ok(host) = super::host::Host::new(true) else {
            eprintln!("no software adapter; the dispatch is skipped");
            return;
        };
        let cs = super::HostShader {
            stage: super::HwStage::Cs,
            dxil,
            user_data: l.user_data,
            samplers: Vec::new(),
            static_samplers: Vec::new(),
            root_cbvs: l.root_cbvs,
            root_srvs: l.root_srvs,
            root_uavs: l.root_uavs,
            root_indexed_views: Vec::new(),
        };
        let (root, pso) = host.compute_pipeline(&cs).unwrap();
        let views: Vec<super::host::View> = if raw {
            vec![
                super::host::View::Cbv(&other),
                super::host::View::RawUav(8),
                super::host::View::Buffer { data: &data, stride: super::host::Stride::Raw },
                super::host::View::RawUav(8),
            ]
        } else {
            vec![super::host::View::Cbv(&data), super::host::View::RawUav(8)]
        };
        let out = host
            .dispatch(
                &root,
                &pso,
                &super::host::Dispatch {
                    roots: vec![(0, super::host::Root::Constants(vec![s0, 0, 0, 0, 0]))],
                    views: &views,
                    samplers: &[],
                    groups: [1, 1, 1],
                },
            )
            .unwrap();
        // The UAV is written where the program reads it: its twin with the switch.
        let got = out.iter().find(|o| o[..5].iter().any(|&w| w != 0)).map(|o| o[..5].to_vec());
        assert_eq!(got.as_deref(), Some(&want[..]), "switch {raw}: {out:x?}");
    }
}
