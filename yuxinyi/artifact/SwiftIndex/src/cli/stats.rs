use anyhow::Result;

use crate::{cli::StatsArgs, query};

pub fn handle(args: StatsArgs) -> Result<()> {
    query::stats::run(args)
}
