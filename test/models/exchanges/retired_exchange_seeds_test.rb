require 'test_helper'

# A retired venue must not come back on the next fresh install. Both seed paths (db/seeds.rb for
# the app, lib/tasks/seed.rake for the container bootstrap) list exchanges by hand, so this is the
# only place that would silently re-create the row.
class RetiredExchangeSeedsTest < ActiveSupport::TestCase
  test 'no seed path creates a retired exchange' do
    [Rails.root.join('db/seeds.rb'), Rails.root.join('lib/tasks/seed.rake')].each do |path|
      contents = path.read

      Exchange::RETIRED_TYPES.each do |type|
        refute_includes contents, type, "#{path.basename} still seeds #{type}"
      end
    end
  end

  test 'no seeded ticker file exists for a retired exchange' do
    Exchange::RETIRED_TYPES.each do |type|
      name_id = type.demodulize.underscore
      refute Rails.root.join("db/seed_data/tickers/#{name_id}.json").exist?,
             "db/seed_data/tickers/#{name_id}.json should be gone"
    end
  end

  test 'seeded index availability carries no retired exchange' do
    data = JSON.parse(Rails.root.join('db/seed_data/indices.json').read)

    Array(data['data']).each do |index|
      Exchange::RETIRED_TYPES.each do |type|
        refute_includes Array(index['available_exchanges']&.keys), type
        refute_includes Array(index['top_coins_by_exchange']&.keys), type
      end
    end
  end
end
