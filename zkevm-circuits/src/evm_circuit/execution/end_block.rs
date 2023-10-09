use crate::{
    evm_circuit::{
        execution::ExecutionGadget,
        step::ExecutionState,
        util::{
            constraint_builder::{
                ConstrainBuilderCommon, EVMConstraintBuilder, StepStateTransition, Transition::Same,
            },
            math_gadget::{IsEqualGadget, IsZeroGadget},
            not, CachedRegion, Cell,
        },
        witness::{Block, Call, ExecStep, Transaction},
    },
    table::{CallContextFieldTag, TxContextFieldTag},
    util::{word::Word, Expr},
};
use eth_types::Field;
use gadgets::util::select;
use halo2_proofs::{circuit::Value, plonk::Error};

#[derive(Clone, Debug)]
pub(crate) struct EndBlockGadget<F> {
    total_txs: Cell<F>,
    total_txs_is_max_txs: IsEqualGadget<F>,
    is_empty_block: IsZeroGadget<F>,
    max_rws: Cell<F>,
    max_txs: Cell<F>,
}

impl<F: Field> ExecutionGadget<F> for EndBlockGadget<F> {
    const NAME: &'static str = "EndBlock";

    const EXECUTION_STATE: ExecutionState = ExecutionState::EndBlock;

    fn configure(cb: &mut EVMConstraintBuilder<F>) -> Self {
        let max_txs = cb.query_copy_cell();
        let max_rws = cb.query_copy_cell();
        let total_txs = cb.query_cell();
        let total_txs_is_max_txs = IsEqualGadget::construct(cb, total_txs.expr(), max_txs.expr());
        // Note that rw_counter starts at 1
        let is_empty_block =
            IsZeroGadget::construct(cb, cb.curr.state.rw_counter.clone().expr() - 1.expr());

        let _total_rws_before_padding = cb.curr.state.rw_counter.clone().expr() - 1.expr()
            + select::expr(
                is_empty_block.expr(),
                0.expr(),
                1.expr(), // If the block is not empty, we will do 1 call_context lookup below
            );

        // 1. Constraint total_rws and total_txs witness values depending on the empty
        // block case.
        cb.condition(is_empty_block.expr(), |cb| {
            // 1a.
            cb.require_equal("total_txs is 0 in empty block", total_txs.expr(), 0.expr());
        });
        cb.condition(not::expr(is_empty_block.expr()), |cb| {
            // 1b. total_txs matches the tx_id that corresponds to the final step.
            cb.call_context_lookup_read(
                None,
                CallContextFieldTag::TxId,
                Word::from_lo_unchecked(total_txs.expr()),
            );
        });

        // 2. If total_txs == max_txs, we know we have covered all txs from the
        // tx_table. If not, we need to check that the rest of txs in the
        // table are padding.
        cb.condition(not::expr(total_txs_is_max_txs.expr()), |cb| {
            // Verify that there are at most total_txs meaningful txs in the tx_table, by
            // showing that the Tx following the last processed one has
            // CallerAddress = 0x0 (which means padding tx).
            cb.tx_context_lookup(
                total_txs.expr() + 1.expr(),
                TxContextFieldTag::CallerAddress,
                None,
                Word::zero(),
            );
            // Since every tx lookup done in the EVM circuit must succeed
            // and uses a unique tx_id, we know that at
            // least there are total_tx meaningful txs in
            // the tx_table. We conclude that the number of
            // meaningful txs in the tx_table is total_tx.
        });

        // TODO fix below logic checking logic
        // - startop only exist in first chunk
        // - total_rws_before_padding are across chunk. We need new way, maybe new
        //   `rw_counter_intra_chunk` to lookup padding logic

        // 3. Verify rw_counter counts to the same number of meaningful rows in
        // rw_table to ensure there is no malicious insertion.
        // Verify that there are at most total_rws meaningful entries in the rw_table
        // TODO only lookup start on first rwtable chunk
        cb.rw_table_start_lookup(1.expr());
        // current SRS size < 2^30 so use 4 bytes (2^32) in LtGadet should be enough
        // TODO find better way other than hardcode
        // let is_end_padding_exist = LtGadget::<_, 32>::construct(
        //     cb,
        //     1.expr(),
        //     max_rws.expr() - total_rws_before_padding.expr(),
        // );
        // cb.debug_expression(
        //     "is_end_padding_exist",
        //     max_rws.expr() - total_rws_before_padding.expr(),
        // );
        // cb.condition(is_end_padding_exist.expr(), |cb| {
        //     cb.rw_table_padding_lookup(total_rws_before_padding.expr() + 1.expr());
        //     cb.rw_table_padding_lookup(max_rws.expr());
        // });
        // Since every lookup done in the EVM circuit must succeed and uses
        // a unique rw_counter, we know that at least there are
        // total_rws meaningful entries in the rw_table.
        // We conclude that the number of meaningful entries in the rw_table
        // is total_rws.

        // cb.step_last(|cb| {
        //     // TODO: Handle reward to coinbase.  Depends on spec:
        //     // https://github.com/privacy-scaling-explorations/zkevm-specs/issues/290
        // });
        cb.not_step_last(|cb| {
            // Propagate rw_counter and call_id all the way down.
            cb.require_step_state_transition(StepStateTransition {
                rw_counter: Same,
                call_id: Same,
                ..StepStateTransition::any()
            });
        });

        Self {
            max_txs,
            max_rws,
            total_txs,
            total_txs_is_max_txs,
            is_empty_block,
        }
    }

    fn assign_exec_step(
        &self,
        region: &mut CachedRegion<'_, '_, F>,
        offset: usize,
        block: &Block<F>,
        _: &Transaction,
        _: &Call,
        step: &ExecStep,
    ) -> Result<(), Error> {
        self.is_empty_block
            .assign(region, offset, F::from(u64::from(step.rwc) - 1))?;
        let max_rws = F::from(block.circuits_params.max_rws as u64);
        let max_rws_assigned = self.max_rws.assign(region, offset, Value::known(max_rws))?;

        let total_txs = F::from(block.txs.len() as u64);
        let max_txs = F::from(block.circuits_params.max_txs as u64);
        self.total_txs
            .assign(region, offset, Value::known(total_txs))?;
        self.total_txs_is_max_txs
            .assign(region, offset, total_txs, max_txs)?;
        let max_txs_assigned = self.max_txs.assign(region, offset, Value::known(max_txs))?;
        // When rw_indices is not empty, means current endblock is non-padding step, we're at the
        // last row (at a fixed offset), where we need to access the max_rws and max_txs
        // constant.
        if step.rw_indices_len() != 0 {
            region.constrain_constant(max_rws_assigned, max_rws)?;
            region.constrain_constant(max_txs_assigned, max_txs)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod test {
    use crate::test_util::CircuitTestBuilder;

    use eth_types::bytecode;

    use mock::TestContext;

    fn test_circuit(evm_circuit_pad_to: usize) {
        let bytecode = bytecode! {
            PUSH1(0)
            STOP
        };

        let ctx = TestContext::<2, 1>::simple_ctx_with_bytecode(bytecode).unwrap();

        // finish required tests using this witness block
        CircuitTestBuilder::<2, 1>::new_from_test_ctx(ctx)
            .block_modifier(Box::new(move |block| {
                block.circuits_params.max_evm_rows = evm_circuit_pad_to
            }))
            .run();
    }

    // Test where the EVM circuit contains an exact number of rows corresponding to
    // the trace steps + 1 EndBlock
    #[test]
    fn end_block_exact() {
        test_circuit(0);
    }

    // Test where the EVM circuit has a fixed size and contains several padding
    // EndBlocks at the end after the trace steps
    #[test]
    fn end_block_padding() {
        test_circuit(50);
    }
}
