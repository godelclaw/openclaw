use vstd::prelude::*;

verus! {

#[derive(PartialEq, Eq)]
pub enum IngressMode {
    Off,
    Gate,
    Driver,
}

#[derive(PartialEq, Eq)]
pub enum RouteTarget {
    Control,
    Driver,
    Fallback,
}

pub open spec fn route(mode: IngressMode, is_control_command: bool) -> RouteTarget {
    if is_control_command {
        RouteTarget::Control
    } else {
        match mode {
            IngressMode::Driver => RouteTarget::Driver,
            IngressMode::Off => RouteTarget::Fallback,
            IngressMode::Gate => RouteTarget::Fallback,
        }
    }
}

proof fn lemma_control_always_routes_to_control(mode: IngressMode)
    ensures
        route(mode, true) == RouteTarget::Control,
{
}

proof fn lemma_gate_and_off_never_route_to_driver()
    ensures
        route(IngressMode::Off, false) == RouteTarget::Fallback,
        route(IngressMode::Gate, false) == RouteTarget::Fallback,
{
}

proof fn lemma_driver_routes_to_driver_for_non_control()
    ensures
        route(IngressMode::Driver, false) == RouteTarget::Driver,
{
}

fn main() {}

} // verus!
