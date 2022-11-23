use crate::evm_circuit::util::{
    constraint_builder::ConstraintBuilder, from_bytes, CachedRegion, Cell,
};
use eth_types::Field;
use halo2_proofs::{
    circuit::Value,
    plonk::{Error, Expression},
};

/// Requires that the passed in value is within the specified range.
/// `N_BYTES` is required to be `<= MAX_N_BYTES_INTEGER`.
#[derive(Clone, Debug)]
pub struct RangeCheckGadget<F, const N_BYTES: usize> {
    parts: [Cell<F>; N_BYTES],
}

impl<F: Field, const N_BYTES: usize> RangeCheckGadget<F, N_BYTES> {
    pub(crate) fn construct(cb: &mut ConstraintBuilder<F>, value: Expression<F>) -> Self {
        let parts = cb.query_bytes();

        // Require that the reconstructed value from the parts equals the
        // original value
        cb.require_equal(
            "Constrain bytes recomposited to value",
            value,
            from_bytes::expr(&parts),
        );

        Self { parts }
    }

    pub(crate) fn assign(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        value: F,
    ) -> Result<(), Error> {
        let bytes = value.to_repr();
        for (idx, part) in self.parts.iter().enumerate() {
            part.assign(region, offset, Value::known(F::from(bytes[idx] as u64)))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_util::*;
    use super::*;
    use eth_types::*;
    use gadgets::util::Expr;
    use halo2_proofs::circuit::Value;
    use halo2_proofs::halo2curves::bn256::Fr;
    use halo2_proofs::plonk::Error;

    #[derive(Clone)]
    /// a in [0..1<<32]
    struct RangeCheckTestContainer<F> {
        range_check_gadget: RangeCheckGadget<F, 4>,
        a: Cell<F>,
    }

    impl<F: Field> MathGadgetContainer<F> for RangeCheckTestContainer<F> {
        const NAME: &'static str = "RangeCheckGadget";

        fn configure_gadget_container(cb: &mut ConstraintBuilder<F>) -> Self {
            let a = cb.query_cell();
            let range_check_gadget = RangeCheckGadget::<F, 4>::construct(cb, a.expr());
            RangeCheckTestContainer {
                range_check_gadget,
                a,
            }
        }

        fn assign_gadget_container(
            &self,
            input_words: &[Word],
            region: &mut CachedRegion<'_, '_, F>,
        ) -> Result<(), Error> {
            let a = input_words[0].to_scalar().unwrap();
            let offset = 0;

            self.a.assign(region, offset, Value::known(a))?;
            self.range_check_gadget.assign(region, 0, a)?;

            Ok(())
        }
    }

    #[test]
    fn test_rangecheck_just_in_range() {
        test_math_gadget_container::<Fr, RangeCheckTestContainer<Fr>>(vec![Word::from(0)], true);

        test_math_gadget_container::<Fr, RangeCheckTestContainer<Fr>>(vec![Word::from(1)], true);
        // max - 1
        test_math_gadget_container::<Fr, RangeCheckTestContainer<Fr>>(
            vec![Word::from((1u64 << 32) - 1)],
            true,
        );
    }

    #[test]
    fn test_rangecheck_out_of_range() {
        test_math_gadget_container::<Fr, RangeCheckTestContainer<Fr>>(
            vec![Word::from(1u64 << 32)],
            false,
        );
    }
}
