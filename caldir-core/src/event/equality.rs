use super::{Attachment, XProperty};

// Order-independent compare: BTreeMap-parsed ICS files give alphabetical
// order, but providers (e.g. Google) build x_properties in insertion order.
pub(crate) fn x_properties_eq(a: &[XProperty], b: &[XProperty]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.contains(x))
}

// The URI is the attachment's identity. Compare on it alone (order-independent)
// so adding/removing/changing an attachment registers as a content change,
// while volatile params (SIZE, MANAGED-ID) drifting doesn't cause sync churn.
pub(crate) fn attachments_eq(a: &[Attachment], b: &[Attachment]) -> bool {
    a.len() == b.len() && a.iter().all(|x| b.iter().any(|y| y.uri == x.uri))
}
