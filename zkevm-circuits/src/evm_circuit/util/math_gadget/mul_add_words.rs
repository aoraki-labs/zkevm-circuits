use super::CachedRegion;
use crate::{
    evm_circuit::util::{
        self, constraint_builder::ConstraintBuilder, from_bytes, pow_of_two_expr, split_u256,
        split_u256_limb64, Cell,
    },
    util::Expr,
};
use eth_types::{Field, ToLittleEndian, Word};
use halo2_proofs::{
    circuit::Value,
    plonk::{Error, Expression},
};

/// Construct the gadget that checks a * b + c == d (modulo 2**256),
/// where a, b, c, d are 256-bit words. This can be used by opcode MUL, DIV,
/// and MOD. For opcode MUL, set c to 0. For opcode DIV and MOD, treat c as
/// residue and d as dividend.
///
/// We execute a multi-limb multiplication as follows:
/// a and b is divided into 4 64-bit limbs, denoted as a0~a3 and b0~b3
/// defined t0, t1, t2, t3
///   t0 = a0 * b0, contribute to 0 ~ 128 bit
///   t1 = a0 * b1 + a1 * b0, contribute to 64 ~ 193 bit (include the carry)
///   t2 = a0 * b2 + a2 * b0 + a1 * b1, contribute to above 128 bit
///   t3 = a0 * b3 + a3 * b0 + a2 * b1 + a1 * b2, contribute to above 192 bit
///
/// so t0 ~ t1 include all contributions to the low 256-bit of product, with
/// a maximum 68-bit radix (the part higher than 256-bit), denoted as carry_hi
/// Similarly, we define carry_lo as the radix of contributions to the low
/// 128-bit of the product.
/// We can slightly relax the constraint of carry_lo/carry_hi to 72-bit and
/// allocate 9 bytes for them each
///
/// Finally we just prove:
///   t0 + t1 * 2^64 = <low 128 bit of product> + carry_lo
///   t2 + t3 * 2^64 + carry_lo = <high 128 bit of product> + carry_hi
///
/// Last, we sum the parts that are higher than 256-bit in the multiplication
/// into overflow
///   overflow = carry_hi + a1 * b3 + a2 * b2 + a3 * b1 + a2 * b3 + a3 * b2
///              + a3 * b3
/// In the cases of DIV and MOD, we need to constrain overflow == 0 outside the
/// MulAddWordsGadget.
#[derive(Clone, Debug)]
pub(crate) struct MulAddWordsGadget<F> {
    carry_lo: [Cell<F>; 9],
    carry_hi: [Cell<F>; 9],
    overflow: Expression<F>,
}

impl<F: Field> MulAddWordsGadget<F> {
    pub(crate) fn construct(cb: &mut ConstraintBuilder<F>, words: [&util::Word<F>; 4]) -> Self {
        let (a, b, c, d) = (words[0], words[1], words[2], words[3]);
        let carry_lo = cb.query_bytes();
        let carry_hi = cb.query_bytes();
        let carry_lo_expr = from_bytes::expr(&carry_lo);
        let carry_hi_expr = from_bytes::expr(&carry_hi);

        let mut a_limbs = vec![];
        let mut b_limbs = vec![];
        for trunk in 0..4 {
            let idx = (trunk * 8) as usize;
            a_limbs.push(from_bytes::expr(&a.cells[idx..idx + 8]));
            b_limbs.push(from_bytes::expr(&b.cells[idx..idx + 8]));
        }
        let c_lo = from_bytes::expr(&c.cells[0..16]);
        let c_hi = from_bytes::expr(&c.cells[16..32]);
        let d_lo = from_bytes::expr(&d.cells[0..16]);
        let d_hi = from_bytes::expr(&d.cells[16..32]);

        let t0 = a_limbs[0].clone() * b_limbs[0].clone();
        let t1 = a_limbs[0].clone() * b_limbs[1].clone() + a_limbs[1].clone() * b_limbs[0].clone();
        let t2 = a_limbs[0].clone() * b_limbs[2].clone()
            + a_limbs[1].clone() * b_limbs[1].clone()
            + a_limbs[2].clone() * b_limbs[0].clone();
        let t3 = a_limbs[0].clone() * b_limbs[3].clone()
            + a_limbs[1].clone() * b_limbs[2].clone()
            + a_limbs[2].clone() * b_limbs[1].clone()
            + a_limbs[3].clone() * b_limbs[0].clone();
        let overflow = carry_hi_expr.clone()
            + a_limbs[1].clone() * b_limbs[3].clone()
            + a_limbs[2].clone() * b_limbs[2].clone()
            + a_limbs[3].clone() * b_limbs[2].clone()
            + a_limbs[2].clone() * b_limbs[3].clone()
            + a_limbs[3].clone() * b_limbs[2].clone()
            + a_limbs[3].clone() * b_limbs[3].clone();

        cb.require_equal(
            "(a * b)_lo + c_lo == d_lo + carry_lo ⋅ 2^128",
            t0.expr() + t1.expr() * pow_of_two_expr(64) + c_lo,
            d_lo + carry_lo_expr.clone() * pow_of_two_expr(128),
        );
        cb.require_equal(
            "(a * b)_hi + c_hi + carry_lo == d_hi + carry_hi ⋅ 2^128",
            t2.expr() + t3.expr() * pow_of_two_expr(64) + c_hi + carry_lo_expr,
            d_hi + carry_hi_expr * pow_of_two_expr(128),
        );

        Self {
            carry_lo,
            carry_hi,
            overflow,
        }
    }

    pub(crate) fn assign(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        words: [Word; 4],
    ) -> Result<(), Error> {
        let (a, b, c, d) = (words[0], words[1], words[2], words[3]);

        let a_limbs = split_u256_limb64(&a);
        let b_limbs = split_u256_limb64(&b);
        let (c_lo, c_hi) = split_u256(&c);
        let (d_lo, d_hi) = split_u256(&d);

        let t0 = a_limbs[0] * b_limbs[0];
        let t1 = a_limbs[0] * b_limbs[1] + a_limbs[1] * b_limbs[0];
        let t2 = a_limbs[0] * b_limbs[2] + a_limbs[1] * b_limbs[1] + a_limbs[2] * b_limbs[0];
        let t3 = a_limbs[0] * b_limbs[3]
            + a_limbs[1] * b_limbs[2]
            + a_limbs[2] * b_limbs[1]
            + a_limbs[3] * b_limbs[0];

        let carry_lo = (t0 + (t1 << 64) + c_lo - d_lo) >> 128;
        let carry_hi = (t2 + (t3 << 64) + c_hi + carry_lo - d_hi) >> 128;

        self.carry_lo
            .iter()
            .zip(carry_lo.to_le_bytes().iter())
            .map(|(cell, byte)| cell.assign(region, offset, Value::known(F::from(*byte as u64))))
            .collect::<Result<Vec<_>, _>>()?;

        self.carry_hi
            .iter()
            .zip(carry_hi.to_le_bytes().iter())
            .map(|(cell, byte)| cell.assign(region, offset, Value::known(F::from(*byte as u64))))
            .collect::<Result<Vec<_>, _>>()?;

        Ok(())
    }

    pub(crate) fn overflow(&self) -> Expression<F> {
        self.overflow.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use eth_types::Word;
    use halo2_proofs::halo2curves::bn256::Fr;
    use halo2_proofs::plonk::Error;

    #[test]
    fn test_muladd() {
        #[derive(Clone)]
        /// a*b + c == d
        struct MulAddGadgetContainer<F> {
            math_gadget: MulAddWordsGadget<F>,
            a: util::Word<F>,
            b: util::Word<F>,
            c: util::Word<F>,
            d: util::Word<F>,
        }

        impl<F: Field> MathGadgetContainer<F> for MulAddGadgetContainer<F> {
            const NAME: &'static str = "MulAddGadget";

            fn configure_gadget_container(cb: &mut ConstraintBuilder<F>) -> Self {
                let a = cb.query_word();
                let b = cb.query_word();
                let c = cb.query_word();
                let d = cb.query_word();
                let math_gadget = MulAddWordsGadget::<F>::construct(cb, [&a, &b, &c, &d]);
                MulAddGadgetContainer {
                    math_gadget,
                    a,
                    b,
                    c,
                    d,
                }
            }

            fn assign_gadget_container(
                &self,
                input_words: &[Word],
                region: &mut CachedRegion<'_, '_, F>,
            ) -> Result<(), Error> {
                let offset = 0;
                self.a
                    .assign(region, offset, Some(input_words[0].to_le_bytes()))?;
                self.b
                    .assign(region, offset, Some(input_words[1].to_le_bytes()))?;
                self.c
                    .assign(region, offset, Some(input_words[2].to_le_bytes()))?;
                self.d
                    .assign(region, offset, Some(input_words[3].to_le_bytes()))?;
                self.math_gadget
                    .assign(region, offset, input_words.try_into().unwrap())
            }
        }

        test_math_gadget_container::<Fr, MulAddGadgetContainer<Fr>>(
            vec![Word::from(0), Word::from(0), Word::from(0), Word::from(0)],
            true,
        );

        test_math_gadget_container::<Fr, MulAddGadgetContainer<Fr>>(
            vec![Word::from(1), Word::from(0), Word::from(0), Word::from(0)],
            true,
        );

        test_math_gadget_container::<Fr, MulAddGadgetContainer<Fr>>(
            vec![Word::from(1), Word::from(1), Word::from(0), Word::from(1)],
            true,
        );

        test_math_gadget_container::<Fr, MulAddGadgetContainer<Fr>>(
            vec![Word::from(1), Word::from(1), Word::from(1), Word::from(2)],
            true,
        );

        test_math_gadget_container::<Fr, MulAddGadgetContainer<Fr>>(
            vec![Word::from(10), Word::from(1), Word::from(1), Word::from(3)],
            false,
        );
    }
}
