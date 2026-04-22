//! Reproduce the first strict phase-failing batch for an experimental prefix
//! length and bisect which top-level cut first shows a phase divergence.
//!
//! This mirrors `main.rs` closely enough to find the actual failing batch:
//! - one Shake instance seeded from the experimental op stream,
//! - point generation and simulator randomness consume that same stream.
//!
//! Then, on that exact 64-shot batch, we compare generic vs experimental phase
//! at a few top-level cuts using a shared deterministic simulator seed.

use std::sync::{Mutex, OnceLock};

use alloy_primitives::U256;
use sha3::{digest::{ExtendableOutput, Update, XofReader}, Shake256};

use crate::circuit::{analyze_ops, BitId, Op, QubitId};
use crate::sim::Simulator;
use crate::weierstrass_elliptic_curve::WeierstrassEllipticCurve;

use super::test_timeout::{check_deadline, two_min_deadline};
use super::{
    B, N, SECP256K1_P, mod_add_double_qb, mod_add_qb, mod_mul_add_into_acc_schoolbook,
    mod_mul_sub_qq, mod_mul_write_into_zero_acc_karatsuba2, mod_mul_write_into_zero_acc_schoolbook,
    mod_neg_inplace_fast, mod_sub_qb, with_kal_inv_raw,
};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_bulk_env<T>(iters: usize, enabled: bool, f: impl FnOnce() -> T) -> T {
    let _guard = env_lock().lock().unwrap();
    let old_exp = std::env::var("KAL_BULK3_EXPERIMENT").ok();
    let old_iters = std::env::var("KAL_BULK3_ITERS").ok();
    unsafe {
        if enabled {
            std::env::set_var("KAL_BULK3_EXPERIMENT", "1");
            std::env::set_var("KAL_BULK3_ITERS", iters.to_string());
        } else {
            std::env::remove_var("KAL_BULK3_EXPERIMENT");
            std::env::remove_var("KAL_BULK3_ITERS");
        }
    }
    let out = f();
    unsafe {
        match old_exp {
            Some(v) => std::env::set_var("KAL_BULK3_EXPERIMENT", v),
            None => std::env::remove_var("KAL_BULK3_EXPERIMENT"),
        }
        match old_iters {
            Some(v) => std::env::set_var("KAL_BULK3_ITERS", v),
            None => std::env::remove_var("KAL_BULK3_ITERS"),
        }
    }
    out
}

fn secp256k1() -> WeierstrassEllipticCurve {
    WeierstrassEllipticCurve {
        modulus: U256::from_str_radix("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFC2F", 16).unwrap(),
        a: U256::from(0),
        b: U256::from(7),
        gx: U256::from_str_radix("79BE667EF9DCBBAC55A06295CE870B07029BFCDB2DCE28D959F2815B16F81798", 16).unwrap(),
        gy: U256::from_str_radix("483ADA7726A3C4655DA4FBFC0E1108A8FD17B448A68554199C47D08FFB10D4B8", 16).unwrap(),
        order: U256::from_str_radix("FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEBAAEDCE6AF48A03BBFD25E8CD0364141", 16).unwrap(),
    }
}

fn fiat_shamir_seed(ops: &[Op]) -> sha3::Shake256Reader {
    let mut hasher = Shake256::default();
    hasher.update(b"quantum_ecc-fiat-shamir-v1");
    hasher.update(&(ops.len() as u64).to_le_bytes());
    for op in ops {
        hasher.update(&[op.kind as u8]);
        hasher.update(&op.q_control2.0.to_le_bytes());
        hasher.update(&op.q_control1.0.to_le_bytes());
        hasher.update(&op.q_target.0.to_le_bytes());
        hasher.update(&op.c_target.0.to_le_bytes());
        hasher.update(&op.c_condition.0.to_le_bytes());
        hasher.update(&op.r_target.0.to_le_bytes());
    }
    hasher.finalize_xof()
}

fn build_full_ops(experiment: bool, bulk_iters: usize) -> Vec<Op> {
    with_bulk_env(bulk_iters, experiment, super::build)
}

#[derive(Clone, Copy)]
enum Cut {
    AfterPair1,
    AfterRxMinusQx,
    AfterMul3,
    BeforeLamFree,
}

#[derive(Clone)]
struct CutCircuit {
    ops: Vec<Op>,
    num_qubits: usize,
    num_bits: usize,
    tx: Vec<QubitId>,
    ty: Vec<QubitId>,
    ox: Vec<BitId>,
    oy: Vec<BitId>,
}

fn build_cut(experiment: bool, bulk_iters: usize, cut: Cut) -> CutCircuit {
    with_bulk_env(bulk_iters, experiment, || {
        let b = &mut B::new();
        let tx = b.alloc_qubits(N);
        let ty = b.alloc_qubits(N);
        let ox = b.alloc_bits(N);
        let oy = b.alloc_bits(N);
        let p = SECP256K1_P;
        let pair1_iters = 2 * N - 113;
        let pair2_iters = 2 * N - 113;

        mod_sub_qb(b, &tx, &ox, p);
        mod_sub_qb(b, &ty, &oy, p);
        let lam = b.alloc_qubits(N);
        with_kal_inv_raw(b, &tx, p, pair1_iters, |b, inv_raw| {
            mod_mul_write_into_zero_acc_schoolbook(b, &lam, &ty, inv_raw, p);
            for _ in 0..pair1_iters { super::mod_halve_inplace_fast(b, &lam, p); }
            mod_mul_add_into_acc_schoolbook(b, &ty, &lam, &tx, p);
        });
        if matches!(cut, Cut::AfterPair1) {
            return CutCircuit { ops: b.ops.clone(), num_qubits: b.next_qubit as usize, num_bits: b.next_bit as usize, tx, ty, ox, oy };
        }

        mod_mul_sub_qq(b, &tx, &lam, &lam, p);
        mod_add_double_qb(b, &tx, &ox, p);
        mod_add_qb(b, &tx, &ox, p);
        mod_neg_inplace_fast(b, &tx, p);
        if matches!(cut, Cut::AfterRxMinusQx) {
            return CutCircuit { ops: b.ops.clone(), num_qubits: b.next_qubit as usize, num_bits: b.next_bit as usize, tx, ty, ox, oy };
        }

        mod_mul_write_into_zero_acc_karatsuba2(b, &ty, &lam, &tx, p);
        if matches!(cut, Cut::AfterMul3) {
            return CutCircuit { ops: b.ops.clone(), num_qubits: b.next_qubit as usize, num_bits: b.next_bit as usize, tx, ty, ox, oy };
        }

        with_kal_inv_raw(b, &tx, p, pair2_iters, |b, inv_raw| {
            for _ in 0..pair2_iters { super::mod_double_inplace_fast(b, &lam, p); }
            mod_mul_add_into_acc_schoolbook(b, &lam, inv_raw, &ty, p);
            mod_sub_qb(b, &ty, &oy, p);
        });
        mod_add_qb(b, &tx, &ox, p);
        debug_assert!(matches!(cut, Cut::BeforeLamFree));
        CutCircuit { ops: b.ops.clone(), num_qubits: b.next_qubit as usize, num_bits: b.next_bit as usize, tx, ty, ox, oy }
    })
}

fn set_qubits<R: sha3::digest::XofReader>(sim: &mut Simulator<R>, qs: &[QubitId], val: U256, shot: usize) {
    for (i, &q) in qs.iter().enumerate() {
        if val.bit(i) { *sim.qubit_mut(q) |= 1u64 << shot; } else { *sim.qubit_mut(q) &= !(1u64 << shot); }
    }
}

fn set_bits<R: sha3::digest::XofReader>(sim: &mut Simulator<R>, bs: &[BitId], val: U256, shot: usize) {
    for (i, &b) in bs.iter().enumerate() {
        if val.bit(i) { *sim.bit_mut(b) |= 1u64 << shot; } else { *sim.bit_mut(b) &= !(1u64 << shot); }
    }
}

fn run_cut_on_batch(c: &CutCircuit, batch_pts: &[((U256,U256),(U256,U256))]) -> u64 {
    let mut hasher = sha3::Shake128::default();
    hasher.update(b"kaliski-phase-bisect-v1");
    let mut xof = hasher.finalize_xof();
    let mut sim = Simulator::new(c.num_qubits, c.num_bits, &mut xof);
    for (shot, &(t, o)) in batch_pts.iter().enumerate() {
        set_qubits(&mut sim, &c.tx, t.0, shot);
        set_qubits(&mut sim, &c.ty, t.1, shot);
        set_bits(&mut sim, &c.ox, o.0, shot);
        set_bits(&mut sim, &c.oy, o.1, shot);
    }
    sim.apply(&c.ops);
    sim.global_phase()
}

fn sampled_batches_for_ops(ops: &[Op], n_batches: usize) -> Vec<Vec<((U256,U256),(U256,U256))>> {
    let curve = secp256k1();
    let mut xof = fiat_shamir_seed(ops);
    let mut batches = Vec::with_capacity(n_batches);
    for _ in 0..n_batches {
        let mut current = Vec::with_capacity(64);
        while current.len() < 64 {
            let mut rb = [[0u8; 32]; 2];
            xof.read(&mut rb[0]);
            xof.read(&mut rb[1]);
            let k1 = U256::from_le_bytes(rb[0]);
            let k2 = U256::from_le_bytes(rb[1]);
            let t = curve.mul(curve.gx, curve.gy, k1);
            let o = curve.mul(curve.gx, curve.gy, k2);
            if t.0 == o.0 { continue; }
            if t.0.is_zero() && t.1.is_zero() { continue; }
            if o.0.is_zero() && o.1.is_zero() { continue; }
            current.push((t, o));
        }
        batches.push(current);
    }
    batches
}

fn first_phase_failing_batch_points(bulk_iters: usize) -> Option<(usize, Vec<((U256,U256),(U256,U256))>)> {
    let ops = build_full_ops(true, bulk_iters);
    let (total_qubits, num_bits, _nregs, regs) = analyze_ops(ops.iter().copied());
    let curve = secp256k1();
    let mut xof = fiat_shamir_seed(&ops);
    let mut current = Vec::with_capacity(64);
    let mut batch_idx = 0usize;
    while batch_idx * 64 < 9024 {
        while current.len() < 64 {
            let mut rb = [[0u8; 32]; 2];
            xof.read(&mut rb[0]);
            xof.read(&mut rb[1]);
            let k1 = U256::from_le_bytes(rb[0]);
            let k2 = U256::from_le_bytes(rb[1]);
            let t = curve.mul(curve.gx, curve.gy, k1);
            let o = curve.mul(curve.gx, curve.gy, k2);
            if t.0 == o.0 { continue; }
            if t.0.is_zero() && t.1.is_zero() { continue; }
            if o.0.is_zero() && o.1.is_zero() { continue; }
            current.push((t, o));
        }
        let mut sim = Simulator::new(total_qubits as usize, num_bits as usize, &mut xof);
        sim.clear_for_shot();
        for shot in 0..64 {
            let (t, o) = current[shot];
            sim.set_register(&regs[0], t.0, shot);
            sim.set_register(&regs[1], t.1, shot);
            sim.set_register(&regs[2], o.0, shot);
            sim.set_register(&regs[3], o.1, shot);
        }
        sim.apply(&ops);
        if sim.global_phase() != 0 {
            return Some((batch_idx, current.clone()));
        }
        current.clear();
        batch_idx += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_bisect_k4_first_failing_batch() {
        let deadline = two_min_deadline();
        let (batch_idx, batch_pts) = first_phase_failing_batch_points(4).expect("expected failing phase batch for k=4");
        check_deadline(deadline, "kaliski_phase_bisect::phase_bisect_k4_first_failing_batch");
        let cuts = [Cut::AfterPair1, Cut::AfterRxMinusQx, Cut::AfterMul3, Cut::BeforeLamFree];
        let labels = ["after_pair1", "after_rx_minus_qx", "after_mul3", "before_lam_free"];

        eprintln!("=== phase bisect on first failing batch, k=4 ===");
        eprintln!("batch_idx = {}", batch_idx);
        for (cut, label) in cuts.iter().zip(labels.iter()) {
            let g = build_cut(false, 0, *cut);
            let s = build_cut(true, 4, *cut);
            let pg = run_cut_on_batch(&g, &batch_pts);
            let ps = run_cut_on_batch(&s, &batch_pts);
            eprintln!("{:<18} generic={:#018x} special={:#018x}", label, pg, ps);
        }
        eprintln!("===============================================");
        assert!(batch_idx < 9024 / 64);
    }

    #[test]
    fn phase_bisect_k4_batch10_exact() {
        let deadline = two_min_deadline();
        let ops_exp = build_full_ops(true, 4);
        let batches_exp = sampled_batches_for_ops(&ops_exp, 11);
        let batch_pts = &batches_exp[10];
        check_deadline(deadline, "kaliski_phase_bisect::phase_bisect_k4_batch10_exact");
        let cuts = [Cut::AfterPair1, Cut::AfterRxMinusQx, Cut::AfterMul3, Cut::BeforeLamFree];
        let labels = ["after_pair1", "after_rx_minus_qx", "after_mul3", "before_lam_free"];

        eprintln!("=== phase bisect on exact batch 10, k=4 ===");
        for (cut, label) in cuts.iter().zip(labels.iter()) {
            let g = build_cut(false, 0, *cut);
            let s = build_cut(true, 4, *cut);
            let pg = run_cut_on_batch(&g, batch_pts);
            let ps = run_cut_on_batch(&s, batch_pts);
            eprintln!("{:<18} generic={:#018x} special={:#018x}", label, pg, ps);
        }
        eprintln!("==========================================");
        assert_eq!(batch_pts.len(), 64);
    }

    #[test]
    fn inspect_first_32_batch_phase_masks_k4() {
        let deadline = two_min_deadline();
        let ops_gen = build_full_ops(false, 0);
        let ops_exp = build_full_ops(true, 4);
        let batches_gen = sampled_batches_for_ops(&ops_gen, 32);
        let batches_exp = sampled_batches_for_ops(&ops_exp, 32);
        let full_gen = build_cut(false, 0, Cut::BeforeLamFree);
        let full_exp = build_cut(true, 4, Cut::BeforeLamFree);

        eprintln!("=== inspect first 32 batch phase masks k=4 ===");
        for i in 0..32usize {
            if (i & 7) == 0 { check_deadline(deadline, "kaliski_phase_bisect::inspect_first_32_batch_phase_masks_k4"); }
            let pg = run_cut_on_batch(&full_gen, &batches_gen[i]);
            let pe = run_cut_on_batch(&full_exp, &batches_exp[i]);
            eprintln!("batch {:>2}: gen={:#018x} exp={:#018x}", i, pg, pe);
        }
        eprintln!("=============================================");
        assert!(true);
    }
}
