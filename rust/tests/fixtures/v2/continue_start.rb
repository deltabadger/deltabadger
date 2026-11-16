# The action_race::continue_* tests' Rails side (common::v2::continue_start): 2.70.21 runs its own record_vectors.rb
# basket fixture and Continue block (script/rust/record_vectors.rb as the image ships it) on a Rust-built install, and
# writes the Continue decisions to /x/continue.json, with the predicate captured before Start changes the status and
# the actual ActionJob timestamp beside its decision label.
require 'json'
require 'active_support/testing/time_helpers'
include ActiveSupport::Testing::TimeHelpers
source = File.read(Rails.root.join('script/rust/record_vectors.rb'))
fixture = source.split("# Alpaca crypto baskets (rust/src/engine/basket.rs)", 2).fetch(1)
fixture = fixture.split("vector_row_id = 0", 2).fetch(0)
eval("# Alpaca crypto baskets (rust/src/engine/basket.rs)" + fixture, TOPLEVEL_BINDING)
record = source.split("continue_row = lambda", 2).fetch(1).split("# The web UI (rust/src/web).", 2).fetch(0)
record = "vectors = {}; continue_row = lambda" + record
record = record.sub('      raise "#{name}: Rails refused the start:', '      within = bot.restarting_within_interval?' + "\n" + '      raise "#{name}: Rails refused the start:')
record = record.sub("'decision' => decision }", "'decision' => decision, 'within' => within, 'job_at' => at&.iso8601(6), 'checkpoint' => bot.next_interval_checkpoint_at.utc.iso8601(6) }")
eval(record + "\nFile.write('/x/continue.json', JSON.generate(vectors.fetch('continue_start')))", TOPLEVEL_BINDING)
