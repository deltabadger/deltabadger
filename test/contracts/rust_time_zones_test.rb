require 'test_helper'

# rust/src/web/time_zones.json is ActiveSupport's zone-name table, embedded by the Rust crate to read
# users.time_zone. When ActiveSupport changes the table, re-run script/rust/record_vectors.rb.
class RustTimeZonesTest < ActiveSupport::TestCase
  test 'the time zone table the rust crate embeds is ActiveSupport\'s' do
    embedded = JSON.parse(Rails.root.join('rust/src/web/time_zones.json').read)

    assert_equal ActiveSupport::TimeZone::MAPPING, embedded
    assert_equal ActiveSupport::TimeZone.all.map(&:name).sort, embedded.keys.sort
  end
end
