require 'test_helper'

class DatabasePoolConfigTest < ActiveSupport::TestCase
  # `primary` is shared by the Puma web thread(s) AND the in-process Solid Queue worker
  # threads (SOLID_QUEUE_IN_PUMA=true): production runs 4 SQ worker threads
  # (config/queue.yml: 2 + 2) plus RAILS_MAX_THREADS web thread(s), all drawing from the
  # SAME `primary` pool. Without explicit headroom the pool is just RAILS_MAX_THREADS
  # (=1 on hosted containers), which starved /health-check during the daily stock sync.
  # Assert EVERY env's `primary` pool carries the intended headroom (RAILS_MAX_THREADS + 5).
  test "every env's primary pool has headroom for web + in-Puma Solid Queue workers" do
    base = ENV.fetch('RAILS_MAX_THREADS', 5).to_i
    expected = base + 5 # web thread(s) + the 4 production SQ worker threads + margin

    # database_configuration renders ERB + resolves the `<<: *default` merge for all envs.
    configs = Rails.application.config.database_configuration

    %w[production development test].each do |env|
      pool = configs.fetch(env).fetch('primary').fetch('pool').to_i
      assert_equal expected, pool,
                   "#{env} primary pool (#{pool}) must be RAILS_MAX_THREADS + 5 (#{expected}); the " \
                   'in-Puma Solid Queue workers share the primary pool with the web thread'
    end
  end
end
