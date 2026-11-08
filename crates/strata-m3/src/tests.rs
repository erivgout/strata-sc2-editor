//! Synthetic fixtures: tiny M3 files built byte by byte.

use super::*;

/// Builds an MD34 file from tagged arrays.
#[derive(Default)]
struct Builder {
    data: Vec<u8>,
    entries: Vec<(&'static [u8; 4], u32, u32, u32)>,
}

const HEADER: usize = 24;

impl Builder {
    /// Adds an array and returns its reference bytes (count, index, flags).
    fn add(&mut self, t: &'static [u8; 4], version: u32, count: u32, bytes: &[u8]) -> [u8; 12] {
        while !self.data.len().is_multiple_of(16) {
            self.data.push(0);
        }
        let offset = (HEADER + self.data.len()) as u32;
        self.data.extend_from_slice(bytes);
        self.entries.push((t, offset, count, version));
        reference(count, self.entries.len() as u32 - 1)
    }

    fn string(&mut self, s: &str) -> [u8; 12] {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        self.add(b"CHAR", 0, b.len() as u32, &b)
    }

    fn finish(mut self, model: [u8; 12]) -> Vec<u8> {
        while !self.data.len().is_multiple_of(16) {
            self.data.push(0);
        }
        let index_offset = HEADER + self.data.len();
        let mut out = Vec::new();
        out.extend_from_slice(&tag(b"MD34"));
        out.extend_from_slice(&(index_offset as u32).to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&model);
        out.extend_from_slice(&self.data);
        for (t, off, count, version) in &self.entries {
            out.extend_from_slice(&tag(t));
            out.extend_from_slice(&off.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&version.to_le_bytes());
        }
        out
    }
}

fn reference(count: u32, index: u32) -> [u8; 12] {
    let mut r = [0u8; 12];
    r[0..4].copy_from_slice(&count.to_le_bytes());
    r[4..8].copy_from_slice(&index.to_le_bytes());
    r
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) {
    buf[at..at + bytes.len()].copy_from_slice(bytes);
}

fn f(v: f32) -> [u8; 4] {
    v.to_le_bytes()
}

/// Options for the fixture.
struct Fixture {
    /// Bone default location (the bind pose is at the origin).
    bone_offset: [f32; 3],
    /// Default of the bone's batching flag.
    batch_visible: bool,
    /// Bind the batch to bone 0.
    batch_bone: bool,
    /// Number of identical (fully overlapping) regions, each with a batch.
    regions: u16,
}

impl Default for Fixture {
    fn default() -> Self {
        Fixture { bone_offset: [0.0; 3], batch_visible: true, batch_bone: false, regions: 1 }
    }
}

const VFLAGS: u32 = 0x182007d;
const STRIDE: usize = 32;

/// A unit quad (two triangles) on the XY plane, skinned to one bone, with a
/// standard material whose diffuse layer is `Assets\Textures\quad.dds`.
fn quad(opt: &Fixture) -> Vec<u8> {
    let mut b = Builder::default();
    let name = b.string("Quad");

    let corners = [[0.0f32, 0.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0], [0.0, 1.0, 0.5]];
    let uvs = [[0i16, 0], [2048, 0], [2048, 2048], [0, 2048]];
    let mut verts = Vec::new();
    for (p, uv) in corners.iter().zip(uvs) {
        let mut v = [0u8; STRIDE];
        put(&mut v, 0, &f(p[0]));
        put(&mut v, 4, &f(p[1]));
        put(&mut v, 8, &f(p[2]));
        v[12] = 255; // weight 0
        v[16] = 0; // lookup 0
        put(&mut v, 20, &[128, 128, 255, 255]); // normal +Z
        put(&mut v, 24, &uv[0].to_le_bytes());
        put(&mut v, 26, &uv[1].to_le_bytes());
        put(&mut v, 28, &[255, 128, 128, 255]); // tangent
        verts.extend_from_slice(&v);
    }
    let vref = b.add(b"U8__", 0, verts.len() as u32, &verts);

    let faces: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    let fref = b.add(b"U16_", 0, 6, &faces);

    let mut regn = [0u8; 48];
    put(&mut regn, 12, &4u32.to_le_bytes()); // vertex count
    put(&mut regn, 20, &6u32.to_le_bytes()); // face count
    put(&mut regn, 24, &1u16.to_le_bytes()); // bone count
    put(&mut regn, 28, &1u16.to_le_bytes()); // bone lookup count
    regn[32] = 1; // lookups used
    put(&mut regn, 40, &f(16.0));
    let regns: Vec<u8> = (0..opt.regions).flat_map(|_| regn).collect();
    let rref = b.add(b"REGN", 5, opt.regions as u32, &regns);

    let mut bats = Vec::new();
    for r in 0..opt.regions {
        let mut bat = [0u8; 14];
        put(&mut bat, 4, &r.to_le_bytes());
        let bone: i16 = if opt.batch_bone { 0 } else { -1 };
        put(&mut bat, 12, &bone.to_le_bytes());
        bats.extend_from_slice(&bat);
    }
    let bref = b.add(b"BAT_", 1, opt.regions as u32, &bats);

    let mut div = [0u8; 52];
    put(&mut div, 0, &fref);
    put(&mut div, 12, &rref);
    put(&mut div, 24, &bref);
    put(&mut div, 48, &1u32.to_le_bytes());
    let dref = b.add(b"DIV_", 2, 1, &div);

    let lookup = b.add(b"U16_", 0, 1, &0u16.to_le_bytes());

    let mut bone = [0u8; 160];
    put(&mut bone, 0, &(-1i32).to_le_bytes());
    put(&mut bone, 20, &(-1i16).to_le_bytes());
    for (k, v) in opt.bone_offset.iter().enumerate() {
        put(&mut bone, 32 + k * 4, &f(*v));
    }
    put(&mut bone, 80, &f(1.0)); // rotation w
    for k in 0..3 {
        put(&mut bone, 112 + k * 4, &f(1.0));
    }
    put(&mut bone, 148, &u32::from(opt.batch_visible).to_le_bytes());
    let boneref = b.add(b"BONE", 1, 1, &bone);
    let identity: Vec<u8> = Mat4::IDENTITY.to_cols_array().iter().flat_map(|v| v.to_le_bytes()).collect();
    let irefref = b.add(b"IREF", 0, 1, &identity);

    let path = b.string("Assets\\Textures\\quad.dds");
    let mut layr = vec![0u8; 464];
    put(&mut layr, 4, &path);
    put(&mut layr, 36, &0xecu32.to_le_bytes());
    let lref = b.add(b"LAYR", 26, 1, &layr);

    let mname = b.string("Mat");
    let mut mat = vec![0u8; 352];
    put(&mut mat, 0, &mname);
    put(&mut mat, 16, &0x8u32.to_le_bytes()); // two-sided
    put(&mut mat, 20, &0u32.to_le_bytes()); // opaque
    mat[40] = 128; // alpha test
    put(&mut mat, 48, &f(1.5));
    put(&mut mat, 64, &lref); // diffuse
    let after = 64 + 18 * 12;
    put(&mut mat, after + 8, &4u32.to_le_bytes()); // team color emissive add
    let mref = b.add(b"MAT_", 20, 1, &mat);
    let mut matm = [0u8; 8];
    put(&mut matm, 0, &1u32.to_le_bytes());
    let matmref = b.add(b"MATM", 0, 1, &matm);

    let layout = Modl::layout(0, 30);
    let mut modl = vec![0u8; layout.size];
    put(&mut modl, 0, &name);
    put(&mut modl, 80, &boneref);
    put(&mut modl, 96, &VFLAGS.to_le_bytes());
    put(&mut modl, 100, &vref);
    put(&mut modl, 112, &dref);
    put(&mut modl, 124, &lookup);
    put(&mut modl, layout.material_refs, &matmref);
    put(&mut modl, layout.materials_standard, &mref);
    put(&mut modl, layout.bone_rests, &irefref);
    let model = b.add(b"MODL", 30, 1, &modl);
    b.finish(model)
}

#[test]
fn modl_sizes_match_known_versions() {
    for (v, size) in [(20, 748), (21, 760), (23, 784), (24, 796), (25, 808), (26, 820), (28, 844), (29, 856), (30, 868)]
    {
        assert_eq!(Modl::layout(0, v).size, size, "MODL v{v}");
    }
}

#[test]
fn vertex_layouts_match_known_formats() {
    // Sizes of the vertex formats listed in m3addon's structures.xml.
    for (flags, size) in [
        (0x182007d, 32),
        (0x182027d, 36),
        (0x186007d, 36),
        (0x18e007d, 40),
        (0x19e027d, 48),
        (0x1820061, 32),
        (0x1860261, 40),
        (0x4182007d, 36),
    ] {
        assert_eq!(VertexLayout::new(flags).stride, size, "format 0x{flags:x}");
    }
    let l = VertexLayout::new(0x182027d);
    assert_eq!((l.normal_u8, l.uv0_i16, l.skin_pairs), (Some(20), Some(28), 4));
}

#[test]
fn parses_a_quad() {
    let m = parse(&quad(&Fixture::default())).unwrap();
    assert_eq!(m.name, "Quad");
    assert_eq!(m.version, 30);
    assert_eq!(m.positions.len(), 4);
    assert_eq!(m.indices, vec![0, 1, 2, 0, 2, 3]);
    assert_eq!(m.triangle_count(), 2);
    assert_eq!(m.positions[2], [2.0, 1.0, 0.0]);
    assert!((m.normals[0][2] - 1.0).abs() < 0.01);
    assert_eq!(m.uvs[1], [1.0, 0.0]);
    assert_eq!(m.uvs[3], [0.0, 1.0]);
    assert_eq!(m.bounds.min, [0.0, 0.0, 0.0]);
    assert_eq!(m.bounds.max, [2.0, 1.0, 0.5]);
    let s = &m.submeshes[0];
    assert_eq!((s.first_index, s.index_count), (0, 6));
    let mat = &s.material;
    assert_eq!(mat.kind, MaterialKind::Standard);
    assert_eq!(mat.name, "Mat");
    assert_eq!(mat.diffuse.as_ref().and_then(|l| l.path.as_deref()), Some("Assets\\Textures\\quad.dds"));
    assert!(mat.emissive.is_none() && mat.alpha_mask.is_none());
    assert!(mat.two_sided() && !mat.unshaded());
    assert_eq!((mat.blend, mat.alpha_test, mat.emissive_blend), (Blend::Opaque, 128, 4));
    assert_eq!(mat.emissive_mult, 1.5);
}

#[test]
fn default_pose_moves_skinned_vertices() {
    let bytes = quad(&Fixture { bone_offset: [10.0, 0.0, 1.0], ..Fixture::default() });
    let posed = parse(&bytes).unwrap();
    assert_eq!(posed.positions[1], [12.0, 0.0, 1.0]);
    let bind = parse_with(&bytes, Pose::Bind).unwrap();
    assert_eq!(bind.positions[1], [2.0, 0.0, 0.0]);
}

#[test]
fn batches_on_hidden_bones_are_skipped() {
    let shown = parse(&quad(&Fixture { batch_bone: true, ..Fixture::default() })).unwrap();
    assert_eq!(shown.submeshes.len(), 1);
    let hidden = parse(&quad(&Fixture { batch_bone: true, batch_visible: false, ..Fixture::default() })).unwrap();
    assert!(hidden.submeshes.is_empty());
}

#[test]
fn overlapping_regions_cannot_expand_without_bound() {
    // Every region covers the same two triangles (6 indices each).
    let ok = parse(&quad(&Fixture { regions: 50, ..Default::default() })).unwrap();
    assert_eq!(ok.indices.len(), 300);
    assert!(parse(&quad(&Fixture { regions: 120, ..Default::default() })).is_err());
}

#[test]
fn rejects_other_files() {
    assert_eq!(parse(b"DDS \0\0\0\0"), Err(M3Error::NotM3));
    assert_eq!(parse(b""), Err(M3Error::NotM3));
    assert!(matches!(parse(&tag(b"MD33")), Err(M3Error::Unsupported(_))));
}

#[test]
fn malformed_input_never_panics() {
    let good = quad(&Fixture::default());
    for n in 0..good.len() {
        let _ = parse(&good[..n]);
    }
    // Deterministic byte corruption across the whole file.
    let mut seed = 0x2545_f491_u32;
    for _ in 0..4000 {
        let mut b = good.clone();
        for _ in 0..4 {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let i = seed as usize % b.len();
            b[i] = (seed >> 8) as u8;
        }
        let _ = parse(&b);
    }
}
