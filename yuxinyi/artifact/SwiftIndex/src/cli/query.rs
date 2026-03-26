use anyhow::Result;

use crate::{cli::QueryArgs, query};

pub fn handle(args: QueryArgs) -> Result<()> {
    query::dispatch(args)
}
