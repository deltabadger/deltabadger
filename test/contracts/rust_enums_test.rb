require 'test_helper'

# rust/src/enums.rs mirrors these integers through rust/tests/fixtures/ruby_vectors.json. Changing an
# enum here means re-running script/rust/record_vectors.rb, and then the Rust test says what to update.
class RustEnumsTest < ActiveSupport::TestCase
  ENUMS = JSON.parse(Rails.root.join('rust/tests/fixtures/ruby_vectors.json').read).fetch('enums')

  test 'the enums the rust crate mirrors are unchanged' do
    current = {
      'bot_status' => Bot.statuses, 'rule_status' => Rule.statuses, 'transaction_status' => Transaction.statuses, 'transaction_side' => Transaction.sides,
      'transaction_order_type' => Transaction.order_types, 'transaction_external_status' => Transaction.external_statuses,
      'api_key_status' => ApiKey.statuses, 'api_key_key_type' => ApiKey.key_types, 'user_otp_module' => User.otp_modules
    }
    assert_equal(ENUMS, current.transform_values { |h| h.transform_keys(&:to_s) })
  end
end
