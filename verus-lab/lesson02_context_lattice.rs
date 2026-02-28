use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq)]
pub enum ContextTier {
    Public,
    Family,
    Private,
}

pub open spec fn rank(c: ContextTier) -> nat {
    match c {
        ContextTier::Public => 0,
        ContextTier::Family => 1,
        ContextTier::Private => 2,
    }
}

pub open spec fn can_read(src: ContextTier, dst: ContextTier) -> bool {
    rank(dst) <= rank(src)
}

pub open spec fn can_write(src: ContextTier, dst: ContextTier) -> bool {
    can_read(src, dst)
}

pub open spec fn can_flow_to(src: ContextTier, dst: ContextTier) -> bool {
    rank(src) <= rank(dst)
}

proof fn lemma_can_write_equals_can_read(src: ContextTier, dst: ContextTier)
    ensures
        can_write(src, dst) <==> can_read(src, dst),
{
}

proof fn lemma_can_read_reflexive(c: ContextTier)
    ensures
        can_read(c, c),
{
}

proof fn lemma_can_read_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        can_read(a, b) && can_read(b, c) ==> can_read(a, c),
{
    if can_read(a, b) && can_read(b, c) {
        assert(rank(c) <= rank(a));
    }
}

proof fn lemma_can_flow_reflexive(c: ContextTier)
    ensures
        can_flow_to(c, c),
{
}

proof fn lemma_can_flow_transitive(a: ContextTier, b: ContextTier, c: ContextTier)
    ensures
        can_flow_to(a, b) && can_flow_to(b, c) ==> can_flow_to(a, c),
{
    if can_flow_to(a, b) && can_flow_to(b, c) {
        assert(rank(a) <= rank(c));
    }
}

proof fn lemma_context_examples()
    ensures
        !can_read(ContextTier::Public, ContextTier::Family),
        !can_read(ContextTier::Public, ContextTier::Private),
        !can_read(ContextTier::Family, ContextTier::Private),
        can_read(ContextTier::Private, ContextTier::Public),
        can_read(ContextTier::Private, ContextTier::Family),
        can_read(ContextTier::Private, ContextTier::Private),
        can_flow_to(ContextTier::Public, ContextTier::Family),
        can_flow_to(ContextTier::Public, ContextTier::Private),
        can_flow_to(ContextTier::Family, ContextTier::Private),
        !can_flow_to(ContextTier::Private, ContextTier::Family),
        !can_flow_to(ContextTier::Private, ContextTier::Public),
{
}

fn main() {}

} // verus!
