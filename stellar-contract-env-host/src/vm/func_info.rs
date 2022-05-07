use super::dispatch;
use crate::Host;
use stellar_contract_env_common::call_macro_with_all_host_functions;
use wasmi::{RuntimeArgs, RuntimeValue};

pub(crate) struct HostFuncInfo {
    pub(crate) mod_id: &'static str,
    pub(crate) field_name: &'static str,
    pub(crate) arity: usize,
    pub(crate) dispatch: fn(&mut Host, RuntimeArgs) -> Result<RuntimeValue, wasmi::Trap>,
}

///////////////////////////////////////////////////////////////////////////////
/// X-macro use: static HOST_FUNCTIONS array of HostFuncInfo
///////////////////////////////////////////////////////////////////////////////

// This is a helper macro that matches simple ident:ty argument list token-trees
// and returns a literal token that is the arity (number of arguments) in the
// list. It is used to convert the supplied token-tree pattern to an arity number
// stored in the HostFuncInfo.
macro_rules! arity_helper {
    { () } => { 0 };
    { ($a0:ident:$t0:ty) } => { 1 };
    { ($a0:ident:$t0:ty, $a1:ident:$t1:ty) } => { 2 };
    { ($a0:ident:$t0:ty, $a1:ident:$t1:ty, $a2:ident:$t2:ty) } => { 3 };
    { ($a0:ident:$t0:ty, $a1:ident:$t1:ty, $a2:ident:$t2:ty, $a3:ident:$t3:ty) } => { 4 };
    { ($a0:ident:$t0:ty, $a1:ident:$t1:ty, $a2:ident:$t2:ty, $a3:ident:$t3:ty, $a4:ident:$t4:ty) } => { 5 };
    { ($a0:ident:$t0:ty, $a1:ident:$t1:ty, $a2:ident:$t2:ty, $a3:ident:$t3:ty, $a4:ident:$t4:ty, $a5:ident:$t5:ty) } => { 6 };
}

// This is a callback macro that pattern-matches the token-tree passed by the
// x-macro (call_macro_with_all_host_functions) and produces a suite of
// dispatch-function definitions.
macro_rules! generate_host_function_infos {
    {
        $(
            // This outer pattern matches a single 'mod' block of the token-tree
            // passed from the x-macro to this macro. It is embedded in a `$()*`
            // pattern-repetition matcher so that it will match all provided
            // 'mod' blocks provided.
            mod $mod_id:ident $mod_str:literal
            {
                $(
                    // This inner pattern matches a single function description
                    // inside a 'mod' block in the token-tree passed from the
                    // x-macro to this macro. It is embedded in a `$()*`
                    // pattern-repetition matcher so that it will match all such
                    // descriptions.
                    { $fn_id:literal, fn $func_id:ident $selfspec:tt $args:tt -> $ret:ty }
                )*
            }
        )*
    }

    =>   // The part of the macro above this line is a matcher; below is its expansion.

    {
        // This macro expands to a single item: a static array of HostFuncInfo, used by
        // two places:
        //
        //   1. The VM WASM-module instantiation step to resolve all import functions to numbers
        //       and typecheck their signatures (represented here by a simple arity number, since
        //       every host function we have just takes N i64 values and returns an i64).
        //
        //   2. The function dispatch path when guest code calls out of the VM, where we
        //      look up the numbered function the guest is requesting in this array and
        //      call its associated dispatch function.
        pub(crate) static HOST_FUNCTIONS: &[HostFuncInfo] =
        &[
           $(
                $(
                    // This generates a HostFuncInfo struct directly
                    // for each function matched in the token-tree (invoking
                    // the arity_helper! macro above to calculate the arity
                    // of each function along the way). It is embedded in two
                    // nested `$()*` pattern-repetition expanders that
                    // correspond to the pattern-repetition matchers in the
                    // match section, but we ignore the structure of the 'mod'
                    // block repetition-level from the outer pattern in the
                    // expansion, flattening all functions from all 'mod' blocks
                    // into the a single array of HostFuncInfo structs.
                    HostFuncInfo {
                        mod_id: $mod_str,
                        field_name: $fn_id,
                        arity: arity_helper!{$args},
                        dispatch: dispatch::$func_id,
                    },
                )*
            )*
        ];
    };
}

// Here we invoke the x-macro passing generate_host_function_infos as its callback macro.
call_macro_with_all_host_functions! { generate_host_function_infos }
