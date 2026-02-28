use vstd::prelude::*;

verus! {

pub open spec fn leq(x: nat, y: nat) -> bool {
    x <= y
}

proof fn lemma_leq_reflexive(x: nat)
    ensures
        leq(x, x),
{
}

proof fn lemma_leq_transitive(x: nat, y: nat, z: nat)
    ensures
        leq(x, y) && leq(y, z) ==> leq(x, z),
{
    if leq(x, y) && leq(y, z) {
        assert(x <= z);
    }
}

proof fn lemma_leq_total(x: nat, y: nat)
    ensures
        leq(x, y) || leq(y, x),
{
    if x <= y {
        assert(leq(x, y));
    } else {
        assert(leq(y, x));
    }
}

fn main() {}

} // verus!
