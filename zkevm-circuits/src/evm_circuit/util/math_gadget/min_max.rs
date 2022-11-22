use super::CachedRegion;
use crate::evm_circuit::util::{constraint_builder::ConstraintBuilder, math_gadget::*, select};
use eth_types::Field;
use halo2_proofs::plonk::{Error, Expression};
/// Returns `rhs` when `lhs < rhs`, and returns `lhs` otherwise.
/// lhs and rhs `< 256**N_BYTES`
/// `N_BYTES` is required to be `<= MAX_N_BYTES_INTEGER`.
#[derive(Clone, Debug)]
pub struct MinMaxGadget<F, const N_BYTES: usize> {
    lt: LtGadget<F, N_BYTES>,
    min: Expression<F>,
    max: Expression<F>,
}

impl<F: Field, const N_BYTES: usize> MinMaxGadget<F, N_BYTES> {
    pub(crate) fn construct(
        cb: &mut ConstraintBuilder<F>,
        lhs: Expression<F>,
        rhs: Expression<F>,
    ) -> Self {
        let lt = LtGadget::construct(cb, lhs.clone(), rhs.clone());
        let max = select::expr(lt.expr(), rhs.clone(), lhs.clone());
        let min = select::expr(lt.expr(), lhs, rhs);

        Self { lt, min, max }
    }

    pub(crate) fn min(&self) -> Expression<F> {
        self.min.clone()
    }

    pub(crate) fn max(&self) -> Expression<F> {
        self.max.clone()
    }

    pub(crate) fn assign(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        lhs: F,
        rhs: F,
    ) -> Result<(F, F), Error> {
        let (lt, _) = self.lt.assign(region, offset, lhs, rhs)?;
        Ok(if lt.is_zero_vartime() {
            (rhs, lhs)
        } else {
            (lhs, rhs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::test_util::*;
    use super::*;
    use crate::evm_circuit::util::Cell;
    use eth_types::{ToScalar, Word};
    use halo2_proofs::halo2curves::bn256::Fr;
    use halo2_proofs::plonk::Error;

    const N: usize = 4;
    #[derive(Clone)]

    struct MinMaxTestContainer<F, const MIN_A: bool> {
        minmax_gadget: MinMaxGadget<F, N>,
        a: Cell<F>,
        b: Cell<F>,
    }

    impl<F: Field, const MIN_A: bool> MathGadgetContainer<F> for MinMaxTestContainer<F, MIN_A> {
        const NAME: &'static str = "MinMaxGadget";

        fn configure_gadget_container(cb: &mut ConstraintBuilder<F>) -> Self {
            let a = cb.query_cell();
            let b = cb.query_cell();
            let minmax_gadget = MinMaxGadget::<F, N>::construct(cb, a.expr(), b.expr());

            if MIN_A {
                cb.require_equal("min == a", minmax_gadget.min(), a.expr());
                cb.require_equal("max == b", minmax_gadget.max(), b.expr());
            } else {
                cb.require_equal("min == b", minmax_gadget.min(), b.expr());
                cb.require_equal("max == a", minmax_gadget.max(), a.expr());
            }

            MinMaxTestContainer {
                minmax_gadget,
                a,
                b,
            }
        }

        fn assign_gadget_container(
            &self,
            input_words: &[Word],
            region: &mut CachedRegion<'_, '_, F>,
        ) -> Result<(), Error> {
            let a = input_words[0].to_scalar().unwrap();
            let b = input_words[1].to_scalar().unwrap();
            let offset = 0;

            self.a.assign(region, offset, Value::known(a))?;
            self.b.assign(region, offset, Value::known(b))?;
            self.minmax_gadget.assign(region, offset, a, b)?;

            Ok(())
        }
    }

    #[test]
    fn test_minmax_eq() {
        test_math_gadget_container::<Fr, MinMaxTestContainer<Fr, true>>(
            vec![Word::from(0), Word::from(0)],
            true,
        );
    }

    #[test]
    fn test_minmax_expect_min_a() {
        test_math_gadget_container::<Fr, MinMaxTestContainer<Fr, true>>(
            vec![Word::from(0), Word::from(1)],
            true,
        );
    }

    #[test]
    fn test_minmx_unexpect_min_a() {
        test_math_gadget_container::<Fr, MinMaxTestContainer<Fr, true>>(
            vec![Word::from(1), Word::from(0)],
            false,
        );
    }

    #[test]
    fn test_minmax_expect_min_b() {
        test_math_gadget_container::<Fr, MinMaxTestContainer<Fr, false>>(
            vec![Word::from(1), Word::from(0)],
            true,
        );
    }
}
