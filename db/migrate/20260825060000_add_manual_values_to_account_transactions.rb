# Values the user states, for the fields the app could not work out for itself.
#
# A JSON column rather than a column per field: the set of fields that can be stated will grow (a
# price today, a fee or a counter-amount tomorrow), and each would otherwise be a migration. Empty
# for all but a handful of rows — the app prices everything it can from its own history, and a
# stated value only exists where it could not, or where the user disagrees.
class AddManualValuesToAccountTransactions < ActiveRecord::Migration[8.1]
  def change
    add_column :account_transactions, :manual_values, :json, default: {}
  end
end
