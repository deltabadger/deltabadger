require 'test_helper'

class AccountTransactionExportTest < ActiveSupport::TestCase
  test 'a hostile exchange description cannot become a spreadsheet formula' do
    tx = create(:account_transaction, description: '=1+1')

    csv = AccountTransaction.to_csv(AccountTransaction.where(id: tx.id))

    refute_includes csv, ',=1+1'
    assert_includes csv, ",'=1+1"
  end
end
