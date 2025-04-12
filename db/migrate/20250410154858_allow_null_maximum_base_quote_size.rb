class AllowNullMaximumBaseQuoteSize < ActiveRecord::Migration[6.0]
  def change
    change_column_null :exchange_tickers, :maximum_base_size, true
    change_column_null :exchange_tickers, :maximum_quote_size, true
  end
end
