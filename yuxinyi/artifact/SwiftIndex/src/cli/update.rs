use anyhow::Result;

use crate::{cli::UpdateArgs, indexer};

pub fn handle(args: UpdateArgs) -> Result<()> {
    indexer::update::run(args)
}
