#!/usr/bin/env python3
"""Remove each collision boundary in isolation; every mutant must fail a regression.
Run from any directory. No production database or network is used. Sources are
restored even on failure. Set the same cargo/Ruby environment as the test suite.
"""
import os
import re
from pathlib import Path
import shutil
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
RAILS = ["bin/rails", "test", "test/models/alpaca_ticker_collision_test.rb",
         "test/jobs/exchange/sync_alpaca_assets_job_test.rb"]
RUST = ["cargo", "test", "--locked", "--test", "reference_jobs", "collision"]
MD = "app/models/market_data.rb"
IMPORT = "rust/src/jobs/import.rs"
SELF = "app/jobs/exchange/sync_alpaca_assets_job.rb"

# A list of exact substitutions per mutant. Multiple edits remove one composite key.
MUTANTS = [
    ("rails display dedup", MD, RAILS, [
        ("[r[:exchange_id], r[:asset_class], r[:base], r[:quote]]", "[r[:exchange_id], r[:base], r[:quote]]")]),
    ("rails display reconciliation", MD, RAILS, [
        ("[t.base_asset.category.to_s, t.base, t.quote]", "[t.base, t.quote]"),
        ("by_base_quote[[record[:asset_class], record[:base], record[:quote]]]", "by_base_quote[[record[:base], record[:quote]]]")]),
    ("rails importing category", MD, RAILS, [
        ("next if category && asset_class != category", "next if false")]),
    ("rails stock importer scope", MD, RAILS, [
        ("import_tickers!(alpaca, listings, category: 'Stock')", "import_tickers!(alpaca, listings)")]),
    ("rails crypto importer scope", MD, RAILS, [
        ("import_tickers!(alpaca, tickers_data, category: 'Cryptocurrency')", "import_tickers!(alpaca, tickers_data)")]),
    ("rails stock count scope", MD, RAILS, [
        ("ticker_records_for(alpaca, listings, category: 'Stock')", "ticker_records_for(alpaca, listings)")]),
    ("rails native holder category", MD, RAILS, [
        ("if holder.base_asset.category.to_s != record[:asset_class]", "if false")]),
    ("rails native batch category", MD, RAILS, [
        ("next if symbol.nil? || group.map { |r| r[:asset_class] }.uniq.one?", "next if true")]),
    ("rails stock sweep", MD, RAILS, [
        ("alpaca.tickers.joins(:base_asset)\n              .where(assets: { category: 'Stock' })", "alpaca.tickers.joins(:base_asset)")]),
    ("rails crypto sweep", MD, RAILS, [
        ("alpaca.tickers.joins(:base_asset)\n              .where(assets: { category: 'Cryptocurrency' })", "alpaca.tickers.joins(:base_asset)")]),
    ("rails direct catalogue category", "app/models/exchange/synchronizer.rb", RAILS, [
        ("stock_venue? ? tickers.joins(:base_asset).where(assets: { category: 'Stock' }) : tickers", "tickers")]),
    ("rails direct discovery category", "app/models/exchange/synchronizer.rb", RAILS, [
        ("next if stock_venue? && base_asset.category != 'Stock'", "next if false")]),
    ("self-hosted sweep category", SELF, RAILS, [
        (".where(assets: { category: category })", ".where('1=1')")]),
    ("rails same-class display dedup", MD, RAILS, [
        ("records.uniq! { |r| [r[:exchange_id], r[:asset_class], r[:base], r[:quote]] }", "# display dedup removed")]),
    ("self-hosted stock native category", SELF, RAILS, [
        ("exchange.tickers.joins(:base_asset).where(assets: { category: 'Stock' }).find_by", "exchange.tickers.find_by")]),
    ("self-hosted crypto native category", SELF, RAILS, [
        ("exchange.tickers.joins(:base_asset).where(assets: { category: 'Cryptocurrency' }).find_by", "exchange.tickers.find_by")]),
    ("self-hosted stock restoration", SELF, RAILS, [
        ("quote_asset: usd_asset, base: stock['symbol'], quote: 'USD', ticker: stock['symbol'], available: true", "quote_asset: usd_asset, available: true")]),
    ("self-hosted crypto restoration", SELF, RAILS, [
        ("quote_asset: usd_asset, base: base, quote: quote, ticker: pair, available: true", "quote_asset: usd_asset, ticker: pair, available: true")]),
    ("rust display dedup", IMPORT, RUST, [
        ("(r.asset_class.clone(), r.base.clone(), r.quote.clone())", "(r.base.clone(), r.quote.clone())")]),
    ("rust display reconciliation", IMPORT, RUST, [
        ("HashMap<(&str, &str, &str), usize>", "HashMap<(&str, &str), usize>"),
        ("(h.asset_class.as_str(), h.base.as_str(), h.quote.as_str())", "(h.base.as_str(), h.quote.as_str())"),
        ("(r.asset_class.as_str(), b.as_str(), q.as_str())", "(b.as_str(), q.as_str())")]),
    ("rust importing category", IMPORT, RUST, [
        ("if category.is_some_and(|category| category != asset_class)", "if false && category.is_some_and(|category| category != asset_class)")]),
    ("rust native holder category", IMPORT, RUST, [
        ("if held[i].asset_class != r.asset_class", "if false && held[i].asset_class != r.asset_class")]),
    ("rust native batch category", IMPORT, RUST, [
        ("if classes.insert(symbol, &r.asset_class).is_some_and(|held| held != &r.asset_class)", "if false && classes.insert(symbol, &r.asset_class).is_some_and(|held| held != &r.asset_class)")]),
    ("rust sweep category", IMPORT, RUST, [
        ("AND a.category = ?2 ORDER BY t.id", "AND ?2 IS NOT NULL ORDER BY t.id")]),
    ("rust stock count category", "rust/src/jobs/reference.rs", RUST, [
        ('ticker_records_for(c, &listings, Some("Stock"))', 'ticker_records_for(c, &listings, None)')]),
    ("rust planner category forwarding", IMPORT, RUST, [
        ("ticker_records_for(c, rows, sweep_category)", "ticker_records_for(c, rows, None)")]),
]

POSITION_RAILS = ["bin/rails", "test", "test/models/alpaca_position_identity_test.rb"]
POSITION_RUST = ["cargo", "test", "--locked", "--test", "sync", "collision_positions"]
ALPACA = "app/models/exchanges/alpaca.rb"
BALANCES = "rust/src/sync/balances.rs"
MUTANTS.extend([
    ("rails position class", ALPACA, POSITION_RAILS, [
        ("position_index.fetch([category, position['symbol']], [])", "position_index.select { |(_, name), _| name == position['symbol'] }.values.flatten")]),
    ("rails live base-only reader", ALPACA, POSITION_RAILS, [
        ("candidates = position_index.fetch([category, position['symbol']], []).map(&:base_asset).uniq(&:id)", "candidates = [asset_from_symbol(position['symbol'])].compact")]),
    ("rails unavailable holdings", ALPACA, POSITION_RAILS, [
        ("tickers.includes(:base_asset, :quote_asset).each_with_object({})", "tickers.available.includes(:base_asset, :quote_asset).each_with_object({})")]),
    ("rails cash class", ALPACA, POSITION_RAILS, [
        ("assets.where(category: Fiat::CATEGORIES, symbol: 'USD')", "assets.where(symbol: 'USD')")]),
    ("rails manual pair ambiguity", "app/services/bot_api/orders/lookup.rb", POSITION_RAILS, [
        ("candidates.first if candidates.one?", "candidates.first")]),
    ("rails bot creation ambiguity", "app/services/bot_api/bots/create_support.rb", POSITION_RAILS, [
        ("return nil unless candidates.one?", "return nil if candidates.empty?")]),
    ("rails order class", ALPACA, POSITION_RAILS, [
        ("index.fetch([category, order_data['symbol']], [])", "index.select { |(_, name), _| name == order_data['symbol'] }.values.flatten")]),
    ("rust order class", "rust/src/engine/model.rs", ["cargo", "test", "--locked", "--test", "tick_alpaca", "collision_untracked"], [
        ("WHERE t.exchange_id=?1 AND a.category=?2", "WHERE t.exchange_id=?1 AND (?2 IS NOT NULL)")]),
    ("rails tracker class", "app/models/tracker/figures.rb", POSITION_RAILS, [
        ("if classes.values.any? { |categories| categories.uniq.size > 1 }", "if false")]),
    ("rust tracker class", "rust/src/tracker/figures.rs", ["cargo", "test", "--locked", "--test", "sync", "collision_tracker"], [
        ("if classes.values().any(|categories| categories.len() > 1)", "if false && classes.values().any(|categories| categories.len() > 1)")]),
    ("rust cash class", BALANCES, POSITION_RUST, [
        ("AND a.category IN ('Fiat','Currency') AND a.symbol='USD'", "AND a.symbol='USD'")]),
    ("rust tombstoned position", BALANCES, POSITION_RUST, [
        ("let native = position_spelling(&native).to_string();", "let native = native.to_string();")]),
    ("rails tombstoned position", ALPACA, POSITION_RAILS, [
        (r"ticker.ticker.sub(/\A__stale_\d+_/, '')", "ticker.ticker")]),
    ("rust position class", BALANCES, POSITION_RUST, [
        ("(category.clone(), name)", "(String::new(), name)"),
        ("get(&(category, symbol))", "get(&(String::new(), symbol))")]),
    ("rust unavailable holdings", BALANCES, POSITION_RUST, [
        ("WHERE t.exchange_id = ?1 ORDER BY t.id", "WHERE t.exchange_id = ?1 AND t.available=1 ORDER BY t.id")]),
])

SETTLEMENT_RAILS = ["bin/rails", "test", "test/models/alpaca_settlement_identity_test.rb"]
SETTLEMENT_RUST = ["cargo", "test", "--locked", "--test", "tick_alpaca", "collision"]
MUTANTS.extend([
    ("rails settlement re-resolution", ALPACA, SETTLEMENT_RAILS, [
        ("resolve_identity: !stored_identity)", "resolve_identity: true)")]),
    ("rails batch settlement re-resolution", ALPACA, SETTLEMENT_RAILS, [
        ("resolve_identity: !stored_identity_ids.include?(order_id)", "resolve_identity: true")]),
    ("rails batch abort", ALPACA, SETTLEMENT_RAILS, [
        ('rescue OrderIdentityError => e\n        Rails.logger.warn("Alpaca order skipped: #{e.message}")', 'rescue OrderIdentityError => e\n        raise e')]),
    ("rails unsupported position abort", ALPACA, SETTLEMENT_RAILS, [
        ("Rails.logger.warn('Alpaca position skipped: unsupported class or unmapped/ambiguous identity')", "return Result::Failure.new('unsupported position')")]),
    ("rust settlement re-resolution", "rust/src/engine/polling.rs", SETTLEMENT_RUST, [
        ("if row.base_asset_id.is_some() && row.quote_asset_id.is_some()", "if false && row.base_asset_id.is_some() && row.quote_asset_id.is_some()")]),
    ("rust batch abort", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('skipped.push(id.clone());\n                continue;', 'return Err(VenueError::Rejected(vec!["unresolved identity".into()]));')]),
    ("rust unsupported position abort", BALANCES, POSITION_RUST, [
        ('return None; }', 'return Some(Err("unsupported position".into())); }')]),
    ("rust unmapped position abort", BALANCES, POSITION_RUST, [
        ('eprintln!("Alpaca position skipped: unmapped or ambiguous identity");\n            continue;', 'return Err("unmapped position".into());')]),
    ("rust funding position abort", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('eprintln!("Alpaca position excluded from cash funding: only the account cash/buying power funds USD orders");', 'return Err(VenueError::Rejected(vec!["unsupported position".into()]));')]),
    ("rails catalogue per position", ALPACA, SETTLEMENT_RAILS, [
        ("candidates = position_index.fetch", "position_index = live_ticker_index; candidates = position_index.fetch")]),
    ("rust order tombstone", "rust/src/engine/model.rs", SETTLEMENT_RUST, [
        ("let native = crate::sync::balances::position_spelling(&native);", "let native = native.as_str();")]),
])


MCP_IDENTITY = ["cargo", "test", "--locked", "--test", "mcp_reads", "collision_mcp"]
MUTANTS.extend([
    ("rust numeric parsing before identity", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('if !identify(id, pair.as_deref(), class.as_deref())? {',
         'let body = http::decode_json(raw.get()).map_err(|e| VenueError::Rejected(vec![unreadable(e, &request, &response)]))?; let _ = parse_order(id, &body).map_err(|e| VenueError::Rejected(vec![e]))?;\n            if !identify(id, pair.as_deref(), class.as_deref())? {')]),
    ("rust status before identity", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('if !identify(id, pair.as_deref(), class.as_deref())? {',
         'let body = http::decode_json(raw.get()).map_err(|e| VenueError::Rejected(vec![unreadable(e, &request, &response)]))?; if status(body["status"].as_str()) == OrderStatus::Unknown { return Err(VenueError::Rejected(vec!["unknown status".into()])); }\n            if !identify(id, pair.as_deref(), class.as_deref())? {')]),
    ("rust MCP parsing before identity", "rust/src/web/mcp/reads.rs", MCP_IDENTITY, [
        ('let Some(ticker)=crate::engine::model::alpaca_order_ticker',
         'let decoded = crate::venue::http::decode_json(raw.get()).map_err(|_|error())?; let _ = crate::venue::alpaca::parse_read_order(id,&decoded).map_err(|_|error())?;\n                        let Some(ticker)=crate::engine::model::alpaca_order_ticker')]),
    ("rust excluded order abandonment", "rust/src/engine/polling.rs", SETTLEMENT_RUST, [
        ('if skipped.contains(ext) || found.iter().any(|o| &o.txid == ext)', 'if found.iter().any(|o| &o.txid == ext)')]),
    ("rust stored identity before parsing", "rust/src/engine/polling.rs", SETTLEMENT_RUST, [
        ('if stored || model::exchange_type', 'if false && stored || model::exchange_type')]),
    ("rust identified numeric leniency", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('out.push(parse_order(id, &body).map_err(|e| VenueError::Rejected(vec![e]))?);',
         'if let Ok(order) = parse_order(id, &body) { out.push(order); }')]),
])

MUTANTS.extend([
    ("rust position decoding before identity", "rust/src/sync/wire.rs", POSITION_RUST, [
        ('identity: true }', 'identity: false }')]),
    ("rust position quantity before catalog identity", BALANCES, POSITION_RUST, [
        ('let ids = catalog.by_symbol.get(&(category, symbol));',
         'let _ = Raw::from_node(&raw)?; let ids = catalog.by_symbol.get(&(category, symbol));')]),
    ("rust venue position decoding before identity", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('let rows = self.raw_positions().await?;',
         'let _ = self.get(self.request("GET", false, "/v2/positions".into(), vec![], None)).await?; let rows = self.raw_positions().await?;')]),
    ("rust funding decoding before identity", "rust/src/venue/alpaca.rs", SETTLEMENT_RUST, [
        ('let positions = self.raw_positions().await?;',
         'let _ = self.get(self.request("GET", false, "/v2/positions".into(), vec![], None)).await?; let positions = self.raw_positions().await?;')]),
    ("rust same-class display dedup", IMPORT, RUST, [
        ('uniq_by(&mut out, |r| (r.asset_class.clone(), r.base.clone(), r.quote.clone()));', '// display dedup removed')]),
])


def check_disk():
    subprocess.run(["df", "-g" if sys.platform == "darwin" else "-k", "/"], check=True)
    if shutil.disk_usage("/").free < 15 * 1024**3:
        raise SystemExit("STOP: fewer than 15 GiB free")


def main():
    if os.environ.get("CARGO_BUILD_JOBS") != "4":
        raise SystemExit("Set CARGO_BUILD_JOBS=4 and the plan's other build environment first")
    logs = ROOT / "tmp" / "ticker-collision-mutations"
    logs.mkdir(parents=True, exist_ok=True)
    for command in (RAILS, RUST, POSITION_RAILS, POSITION_RUST, SETTLEMENT_RAILS, SETTLEMENT_RUST, MCP_IDENTITY):
        check_disk()
        subprocess.run(command, cwd=ROOT / "rust" if command[0] == "cargo" else ROOT, check=True)
    for number, (name, relative, command, edits) in enumerate(MUTANTS, 1):
        check_disk()
        path = ROOT / relative
        original = path.read_bytes()
        mutated = original.decode()
        for before, after in edits:
            if mutated.count(before) != 1:
                raise SystemExit(f"{name}: expected one occurrence of {before!r}")
            mutated = mutated.replace(before, after)
        backup = logs / f"{number:02}.original"
        backup.write_bytes(original)
        try:
            path.write_text(mutated)
            if command[0] == "bin/rails":
                syntax = subprocess.run(["ruby", "-c", str(path)], cwd=ROOT,
                                        stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
                if syntax.returncode != 0:
                    (logs / f"{number:02}.log").write_text(syntax.stdout)
                    raise SystemExit(f"{name}: invalid Ruby syntax; not a killed mutant")
            result = subprocess.run(command, cwd=ROOT / "rust" if command[0] == "cargo" else ROOT,
                                    stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
            (logs / f"{number:02}.log").write_text(result.stdout)
            # A compiler/setup/command failure is not a killed mutant.
            counts = re.search(r"\d+ runs, \d+ assertions, (\d+) failures, (\d+) errors", result.stdout)
            ran = "test result: FAILED" in result.stdout if command[0] == "cargo" else counts and sum(map(int, counts.groups())) > 0
            setup_error = any(marker in result.stdout for marker in
                              ("SyntaxError", "LoadError:", "NameError:", "error[E", "could not compile"))
            if result.returncode == 0 or not ran or setup_error:
                raise SystemExit(f"{name}: survived or never reached assertions; inspect {logs / f'{number:02}.log'}")
            print(f"KILLED {number:02}: {name}", flush=True)
        finally:
            path.write_bytes(original)
            backup.unlink()
    print(f"All {len(MUTANTS)} mutants killed; original sources restored")


if __name__ == "__main__":
    main()
