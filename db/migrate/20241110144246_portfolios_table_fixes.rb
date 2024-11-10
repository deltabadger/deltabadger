class PortfoliosTableFixes < ActiveRecord::Migration[6.0]
  def up
    add_column :portfolios, :temp_date_column, :date, null: false, default: Date.parse('2020-01-01')

    Portfolio.reset_column_information
    Portfolio.find_each do |record|
      record.update_column(:temp_date_column, record.backtest_start_date.to_date)
      if record.limited?
        record.update_column(:smart_allocation_on, false)
      end
    end

    remove_column :portfolios, :backtest_start_date
    rename_column :portfolios, :temp_date_column, :backtest_start_date
    change_column_null :portfolios, :compare_to, false
  end

  def down
    add_column :portfolios, :temp_date_column, :string

    Portfolio.reset_column_information
    Portfolio.find_each do |record|
      record.update_column(:temp_date_column, record.backtest_start_date.to_s)
    end

    remove_column :portfolios, :backtest_start_date
    rename_column :portfolios, :temp_date_column, :backtest_start_date
    change_column_null :portfolios, :compare_to, true
  end
end
