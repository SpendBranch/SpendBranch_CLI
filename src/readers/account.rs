//! The local Claude account: only the organization and the billing type are read, and
//! nothing else in the file (e-mail, name, account id...) is ever stored.
use crate::contract::AccountRef;
use crate::sources::json::{Leaf, Obj};
use serde::Deserialize;
use std::{fs::File, io::BufReader, path::Path};

pub struct LocalAccount {
    pub account: String,
    /// Only with a safe signal (`*_subscription`), never guessed.
    pub subscription: bool,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Top {
    #[serde(rename = "oauthAccount")]
    oauth_account: Obj<Oauth>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Oauth {
    #[serde(rename = "organizationUuid")]
    organization_uuid: Leaf,
    #[serde(rename = "billingType")]
    billing_type: Leaf,
}

pub fn read_local_account(path: &Path) -> Option<LocalAccount> {
    let file = File::open(path).ok()?;
    let Obj(Some(top)) = serde_json::from_reader::<_, Obj<Top>>(BufReader::new(file)).ok()? else {
        return None;
    };
    let oauth = top.oauth_account.0?;
    let account = oauth
        .organization_uuid
        .str()
        .filter(|a| AccountRef::new(a).is_some())?;
    Some(LocalAccount {
        account: account.to_owned(),
        subscription: oauth
            .billing_type
            .str()
            .is_some_and(|b| b.ends_with("_subscription")),
    })
}
