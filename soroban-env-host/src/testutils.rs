use std::cell::Cell;
use std::panic::{catch_unwind, set_hook, take_hook, UnwindSafe};
use std::sync::Once;

use crate::{StorageType, SymbolSmall};
use std::{collections::BTreeMap, rc::Rc};

use rand::RngCore;

use crate::{
    budget::Budget,
    storage::{SnapshotSource, Storage},
    xdr::{
        AccountId, ContractCostType, LedgerEntry, LedgerKey, PublicKey, ScAddress, ScErrorCode,
        ScErrorType, ScVal, ScVec, Uint256,
    },
    AddressObject, BytesObject, Env, EnvBase, Error, Host, HostError, LedgerInfo, Val, VecObject,
};

/// Catch panics while suppressing the default panic hook that prints to the
/// console.
///
/// For the purposes of test reporting we don't want every panicking (but
/// caught) contract call to print to the console. This requires overriding
/// the panic hook, a global resource. This is an awkward thing to do with
/// tests running in parallel.
///
/// This function lazily performs a one-time wrapping of the existing panic
/// hook. It then uses a thread local variable to track contract call depth.
/// If a panick occurs during a contract call the original hook is not
/// called, otherwise it is called.
pub fn call_with_suppressed_panic_hook<C, R>(closure: C) -> std::thread::Result<R>
where
    C: FnOnce() -> R + UnwindSafe,
{
    thread_local! {
        static TEST_CONTRACT_CALL_COUNT: Cell<u64> = Cell::new(0);
    }

    static WRAP_PANIC_HOOK: Once = Once::new();

    WRAP_PANIC_HOOK.call_once(|| {
        let existing_panic_hook = take_hook();
        set_hook(Box::new(move |info| {
            let calling_test_contract = TEST_CONTRACT_CALL_COUNT.with(|c| c.get() != 0);
            if !calling_test_contract {
                existing_panic_hook(info)
            }
        }))
    });

    TEST_CONTRACT_CALL_COUNT.with(|c| {
        let old_count = c.get();
        let new_count = old_count.checked_add(1).expect("overflow");
        c.set(new_count);
    });

    let res = catch_unwind(closure);

    TEST_CONTRACT_CALL_COUNT.with(|c| {
        let old_count = c.get();
        let new_count = old_count.checked_sub(1).expect("overflow");
        c.set(new_count);
    });

    res
}

// Test utilities for the host, used in various tests in sub-modules.
pub trait AsScVal {
    fn as_scval(&self) -> ScVal;
}

impl AsScVal for u32 {
    fn as_scval(&self) -> ScVal {
        ScVal::U32(*self)
    }
}

impl AsScVal for i32 {
    fn as_scval(&self) -> ScVal {
        ScVal::I32(*self)
    }
}

impl AsScVal for u64 {
    fn as_scval(&self) -> ScVal {
        ScVal::U64(*self)
    }
}

impl AsScVal for i64 {
    fn as_scval(&self) -> ScVal {
        ScVal::I64(*self)
    }
}

impl AsScVal for ScVec {
    fn as_scval(&self) -> ScVal {
        ScVal::Vec(Some(self.clone()))
    }
}

pub fn generate_account_id(host: &Host) -> AccountId {
    AccountId(PublicKey::PublicKeyTypeEd25519(Uint256(
        generate_bytes_array(host),
    )))
}

pub fn generate_bytes_array(host: &Host) -> [u8; 32] {
    let mut bytes: [u8; 32] = Default::default();
    host.with_test_prng(|chacha| {
        chacha.fill_bytes(&mut bytes);
        Ok(())
    })
    .unwrap();
    bytes
}

pub struct MockSnapshotSource(BTreeMap<Rc<LedgerKey>, (Rc<LedgerEntry>, Option<u32>)>);

impl MockSnapshotSource {
    pub fn new() -> Self {
        Self(BTreeMap::<Rc<LedgerKey>, (Rc<LedgerEntry>, Option<u32>)>::new())
    }
}
impl SnapshotSource for MockSnapshotSource {
    fn get(&self, key: &Rc<LedgerKey>) -> Result<(Rc<LedgerEntry>, Option<u32>), HostError> {
        if let Some(val) = self.0.get(key) {
            Ok((Rc::clone(&val.0), val.1))
        } else {
            Err(Error::from_type_and_code(ScErrorType::Storage, ScErrorCode::MissingValue).into())
        }
    }

    fn has(&self, key: &Rc<LedgerKey>) -> Result<bool, HostError> {
        Ok(self.0.contains_key(key))
    }
}

impl Host {
    pub const TEST_PRNG_SEED: &'static [u8; 32] = b"12345678901234567890123456789012";

    pub fn test_host() -> Self {
        let host = Host::default();
        host.set_base_prng_seed(*Host::TEST_PRNG_SEED).unwrap();
        host
    }

    pub fn test_host_with_recording_footprint() -> Self {
        let snapshot_source = Rc::<MockSnapshotSource>::new(MockSnapshotSource::new());
        let storage = Storage::with_recording_footprint(snapshot_source);
        let host = Host::with_storage_and_budget(storage, Budget::default());
        host.set_base_prng_seed(*Host::TEST_PRNG_SEED).unwrap();
        host.set_ledger_info(LedgerInfo {
            protocol_version: crate::meta::get_ledger_protocol_version(
                crate::meta::INTERFACE_VERSION,
            ),
            sequence_number: 0,
            timestamp: 0,
            network_id: [0; 32],
            base_reserve: 0,
            min_persistent_entry_ttl: 4096,
            min_temp_entry_ttl: 16,
            max_entry_ttl: 6_312_000,
        })
        .unwrap();
        host
    }

    pub fn test_budget(self, cpu: u64, mem: u64) -> Self {
        self.with_budget(|budget| {
            budget.reset_limits(cpu, mem)?; // something big but finite that we may exceed
            budget.reset_models()?;
            Ok(())
        })
        .unwrap();
        self
    }

    pub fn enable_model(
        self,
        ty: ContractCostType,
        const_cpu: u64,
        lin_cpu: u64,
        const_mem: u64,
        lin_mem: u64,
    ) -> Self {
        self.with_budget(|budget| {
            budget.override_model_with_unscaled_params(ty, const_cpu, lin_cpu, const_mem, lin_mem)
        })
        .unwrap();
        self
    }

    pub fn test_scvec<T: AsScVal>(&self, vals: &[T]) -> Result<ScVec, HostError> {
        let v: Vec<ScVal> = vals.iter().map(|x| x.as_scval()).collect();
        self.map_err(v.try_into())
    }

    pub fn test_vec_obj<T: AsScVal>(&self, vals: &[T]) -> Result<VecObject, HostError> {
        let v = self.test_scvec(vals)?;
        Ok(self.to_host_val(&ScVal::Vec(Some(v)))?.try_into()?)
    }

    pub fn test_vec_val<T: AsScVal>(&self, vals: &[T]) -> Result<Val, HostError> {
        let v = self.test_scvec(vals)?;
        self.to_host_val(&ScVal::Vec(Some(v)))
    }

    pub fn test_bin_scobj(&self, vals: &[u8]) -> Result<ScVal, HostError> {
        Ok(ScVal::Bytes(self.map_err(vals.to_vec().try_into())?))
    }

    pub fn test_bin_obj(&self, vals: &[u8]) -> Result<BytesObject, HostError> {
        let scval: ScVal = self.test_bin_scobj(vals)?;
        let val: Val = self.to_host_val(&scval)?;
        Ok(val.try_into()?)
    }

    // Registers a contract with provided Wasm code and returns the registered
    // contract's address.
    // The contract address deterministically depends on the input account and
    // salt, so this can be used with enforcing ledger footprint (but the
    // footprint still has to be specified outside of this).
    pub fn register_test_contract_wasm_from_source_account(
        &self,
        contract_wasm: &[u8],
        account: AccountId,
        salt: [u8; 32],
    ) -> Result<AddressObject, HostError> {
        let _span = tracy_span!("register_test_contract_wasm_from_source_account");
        // Use source account-based auth in order to avoid using nonces which
        // won't work well with enforcing ledger footprint.
        let prev_source_account = self.source_account_id()?;
        // Use recording auth to skip specifying the auth payload.
        let prev_auth_manager = self.snapshot_auth_manager()?;
        self.switch_to_recording_auth(true)?;

        let wasm_hash = self.upload_wasm(self.bytes_new_from_slice(contract_wasm)?)?;
        self.set_source_account(account.clone())?;
        let contract_address = self.create_contract(
            self.add_host_object(ScAddress::Account(account.clone()))?,
            wasm_hash,
            self.bytes_new_from_slice(&salt)?,
        )?;
        if let Some(prev_account) = prev_source_account {
            self.set_source_account(prev_account)?;
        }
        self.set_auth_manager(prev_auth_manager)?;
        Ok(contract_address)
    }

    // Registers a contract with provided Wasm code and returns the registered
    // contract's address.
    // The contract address will be generated randomly, so this won't work with
    // enforcing ledger footprint.
    pub fn register_test_contract_wasm(&self, contract_wasm: &[u8]) -> AddressObject {
        self.register_test_contract_wasm_from_source_account(
            contract_wasm,
            generate_account_id(self),
            generate_bytes_array(self),
        )
        .unwrap()
    }

    pub fn test_host_with_wasms_and_enforcing_footprint(
        contract_wasms: &[&[u8]],
        contract_zero_extra_read_keys: &[(ScVal, StorageType)],
        contract_zero_extra_write_keys: &[(ScVal, StorageType)],
    ) -> (Host, Vec<AddressObject>) {
        let host = Self::test_host_with_recording_footprint();
        let mut contract_addresses = Vec::new();
        for contract_wasm in contract_wasms.iter() {
            contract_addresses.push(host.register_test_contract_wasm(contract_wasm));
        }
        let ScAddress::Contract(contract_hash) =
            host.scaddress_from_address(contract_addresses[0]).unwrap()
        else {
            panic!()
        };
        let test = SymbolSmall::try_from_str("test").unwrap();
        host.with_test_contract_frame(contract_hash, test.into(), || {
            for (rv, rt) in contract_zero_extra_read_keys {
                let val = host.to_host_val(rv).unwrap();
                host.has_contract_data(val, *rt).unwrap();
            }
            for (wv, wt) in contract_zero_extra_write_keys {
                let val = host.to_host_val(wv).unwrap();
                host.del_contract_data(val, *wt).unwrap();
            }
            Ok(Val::VOID.into())
        })
        .unwrap();
        host.with_mut_storage(|storage| {
            storage.mode = crate::storage::FootprintMode::Enforcing;
            Ok(())
        })
        .unwrap();
        (host, contract_addresses)
    }

    #[cfg(all(test, feature = "testutils"))]
    pub(crate) fn measured_call(
        &self,
        contract: AddressObject,
        func: crate::Symbol,
        args: VecObject,
    ) -> Result<Val, HostError> {
        use crate::{budget::AsBudget, host::HostLifecycleEvent};
        use soroban_bench_utils::HostTracker;
        use std::cell::RefCell;

        let _span = tracy_span!("measured_call");
        let budget = self.as_budget();
        budget.reset_unlimited()?;
        let ht = Rc::new(RefCell::new(HostTracker::new()));

        if std::env::var("EXCLUDE_VM_INSTANTIATION").is_ok() {
            let ht2 = ht.clone();
            let budget2 = budget.clone();
            self.set_lifecycle_event_hook(Some(Rc::new(move |_, evt| {
                if let HostLifecycleEvent::PushCtx(_) = evt {
                    budget2.reset_unlimited()?;
                    ht2.borrow_mut().start(None);
                }
                Ok(())
            })))?;
        } else {
            ht.borrow_mut().start(None);
        }
        let val = self.call(contract, func, args);
        self.set_lifecycle_event_hook(None)?;

        let (cpu_actual, mem_actual, time_nsecs) = Rc::into_inner(ht).unwrap().into_inner().stop();

        let cpu_metered = budget
            .get_cpu_insns_consumed()
            .expect("unable to retrieve cpu consumed");
        let mem_metered = budget
            .get_mem_bytes_consumed()
            .expect("unable to retrieve mem consumed");

        let cpu_diff = (cpu_metered - cpu_actual) as i64;
        let cpu_metered_diff_percent = 100 * cpu_diff / (cpu_metered as i64).max(1);
        let mem_diff = (mem_metered - mem_actual) as i64;
        let mem_metered_diff_percent = 100 * mem_diff / (mem_metered as i64).max(1);
        let metered_insn_nsecs_ratio: f64 = (cpu_metered as f64) / (time_nsecs as f64).max(1.0);
        let actual_insn_nsecs_ratio: f64 = (cpu_actual as f64) / (time_nsecs as f64).max(1.0);

        println!();
        println!(
            "metered cpu insns: {}, actual cpu insns {}, diff: {} ({:.3}%)",
            cpu_metered, cpu_actual, cpu_diff, cpu_metered_diff_percent
        );
        println!(
            "metered mem bytes: {}, actual mem bytes {}, diff: {} ({:.3}%)",
            mem_metered, mem_actual, mem_diff, mem_metered_diff_percent
        );
        println!("time_nsecs: {}", time_nsecs);
        println!(
            "metered cpu_insn/time_nsecs ratio: {:.3}",
            metered_insn_nsecs_ratio
        );
        println!(
            "actual cpu_insn/time_nsecs ratio: {:.3}",
            actual_insn_nsecs_ratio
        );
        println!();

        val
    }
}

#[cfg(test)]
pub(crate) mod wasm {
    use crate::{Symbol, U32Val, Val};
    use soroban_synth_wasm::{Arity, LocalRef, ModEmitter, Operand};

    pub(crate) fn wasm_module_with_4n_insns(n: usize) -> Vec<u8> {
        let mut fe = ModEmitter::default().func(Arity(1), 0);
        let arg = fe.args[0];
        fe.push(Operand::Const64(1));
        for i in 0..n {
            fe.push(arg.0);
            fe.push(Operand::Const64(i as i64));
            fe.i64_mul();
            fe.i64_add();
        }
        fe.drop();
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_mem_grow(n_pages: usize) -> Vec<u8> {
        let mut fe = ModEmitter::default().func(Arity(0), 0);
        fe.push(Operand::Const32(n_pages as i32));
        fe.memory_grow();
        fe.drop();
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_linear_memory_logging() -> Vec<u8> {
        let mut me = ModEmitter::default();
        // log_from_linear_memory
        let f0 = me.import_func("x", "_", Arity(4));
        // the caller
        let mut fe = me.func(Arity(4), 0);
        fe.push(Operand::Local(LocalRef(0)));
        fe.push(Operand::Local(LocalRef(1)));
        fe.push(Operand::Local(LocalRef(2)));
        fe.push(Operand::Local(LocalRef(3)));
        fe.call_func(f0);
        fe.drop();
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_unreachable() -> Vec<u8> {
        let me = ModEmitter::default();
        let mut fe = me.func(Arity(0), 0);
        fe.trap();
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_indirect_call() -> Vec<u8> {
        let mut me = ModEmitter::default();
        // an imported function
        let f0 = me.import_func("t", "_", Arity(0));
        // a local wasm function
        let mut fe = me.func(Arity(0), 0);
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        let (me, f1) = fe.finish();
        // another local wasm function
        let mut fe = me.func(Arity(0), 0);
        fe.push(Symbol::try_from_small_str("pass2").unwrap());
        let (mut me, f2) = fe.finish();
        // store in table
        me.define_elems(&[f0, f1, f2]);
        let ty = me.get_fn_type(Arity(0));
        // the caller
        fe = me.func(Arity(1), 0);
        fe.local_get(LocalRef(0));
        fe.i32_wrap_i64();
        // fe.i32_const(0);
        fe.call_func_indirect(ty);
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_div_by_zero() -> Vec<u8> {
        let me = ModEmitter::default();
        let mut fe = me.func(Arity(0), 0);
        fe.push(Operand::Const64(123));
        fe.push(Operand::Const64(0));
        fe.i64_div_s();
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_integer_overflow() -> Vec<u8> {
        let me = ModEmitter::default();
        let mut fe = me.func(Arity(0), 0);
        fe.push(Operand::Const64(i64::MIN));
        fe.push(Operand::Const64(-1));
        // interestingly the only operation that can trigger `IntegerOverflow`
        // is an overflowing division. Other arithmatic opeartions add, sub, mul
        // are wrapping.
        fe.i64_div_s();
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_user_specified_initial_size(
        mem_pages: u32,
        elem_count: u32,
    ) -> Vec<u8> {
        let me = ModEmitter::from_configs(mem_pages, elem_count);
        // a local wasm function
        let mut fe = me.func(Arity(0), 0);
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_large_data_segment(
        mem_pages: u32,
        mem_offset: u32,
        len: u32,
    ) -> Vec<u8> {
        let mut me = ModEmitter::from_configs(mem_pages, 128);
        me.define_data_segment(mem_offset, vec![0; len as usize]);
        // a local wasm function
        let mut fe = me.func(Arity(0), 0);
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_multiple_data_segments(
        num_pages: u32,
        num_sgmts: u32,
        seg_size: u32,
    ) -> Vec<u8> {
        let mut me = ModEmitter::from_configs(num_pages, 128);
        let mem_len = num_pages * 0x10_000;
        let max_segments = (mem_len / seg_size.max(1)).max(1);
        for _i in 0..num_sgmts % max_segments {
            me.define_data_segment(0, vec![0; seg_size as usize]);
        }
        // a local wasm function
        let mut fe = me.func(Arity(0), 0);
        fe.push(Symbol::try_from_small_str("pass").unwrap());
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_large_bytes_from_linear_memory(
        num_bytes: u32,
        initial: u8,
    ) -> Vec<u8> {
        let num_pages = num_bytes / 0x10_000 + 1;
        let mut me = ModEmitter::from_configs(num_pages, 128);
        me.define_data_segment(0, vec![initial; num_bytes as usize]);
        let mut fe = me.func(Arity(0), 0);
        fe.bytes_new_from_linear_memory(U32Val::from(0), U32Val::from(num_bytes));
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_large_vector_from_linear_memory(
        num_vals: u32,
        initial: Val,
    ) -> Vec<u8> {
        let num_pages = num_vals * 8 / 0x10_000 + 1;
        let mut me = ModEmitter::from_configs(num_pages, 128);
        let bytes: Vec<u8> = (0..num_vals)
            .into_iter()
            .map(|_| initial.get_payload().to_le_bytes())
            .flat_map(|a| a.into_iter())
            .collect();
        me.define_data_segment(0, bytes);
        let mut fe = me.func(Arity(0), 0);
        fe.vec_new_from_linear_memory(U32Val::from(0), U32Val::from(num_vals));
        fe.finish_and_export("test").finish()
    }

    pub(crate) fn wasm_module_with_large_map_from_linear_memory(
        num_vals: u32,
        initial: Val,
    ) -> Vec<u8> {
        // slice is 8 bytes, we choose 8 byte key, val is 8 bytes
        let num_pages = (num_vals * 8) * 3 / 0x10_000 + 1;
        let mut me = ModEmitter::from_configs(num_pages, 128);

        let key_bytes: Vec<u8> = (0..num_vals)
            .into_iter()
            .map(|i| format!("{:0>width$}", i, width = 8))
            .flat_map(|s| s.into_bytes().into_iter())
            .collect();

        let val_bytes: Vec<u8> = (0..num_vals)
            .into_iter()
            .map(|_| initial.get_payload().to_le_bytes())
            .flat_map(|a| a.into_iter())
            .collect();

        let slices: Vec<u8> = (0..num_vals)
            .into_iter()
            .map(|ptr| {
                let slice = 8_u64 << 32 | (ptr * 8) as u64;
                slice.to_le_bytes()
            })
            .flat_map(|s| s.into_iter())
            .collect();

        let bytes: Vec<u8> = key_bytes
            .into_iter()
            .chain(val_bytes.into_iter())
            .chain(slices.into_iter())
            .collect();

        me.define_data_segment(0, bytes);
        let mut fe = me.func(Arity(0), 0);

        let vals_pos = U32Val::from(num_vals * 8);
        let keys_pos = U32Val::from(num_vals * 16);
        fe.map_new_from_linear_memory(keys_pos, vals_pos, U32Val::from(num_vals));
        fe.finish_and_export("test").finish()
    }
}
