class AddVariantsToSubscriptionPlans < ActiveRecord::Migration[6.0]
  def up
    add_column :subscription_plans, :variants, :json, default: []

    # Populate the variants column with data from existing columns
    SubscriptionPlan.reset_column_information # Ensure the model is aware of the new column

    SubscriptionPlan.find_each do |record|
      # Build the new JSON object from existing columns
      variants_data = [{ years: record.years.to_i, cost_eur: record.cost_eu.to_f, cost_usd: record.cost_other.to_f }]

      # Assign the new data to the variants column
      record.update_columns(variants: variants_data)
    end

    # Remove the old columns
    remove_column :subscription_plans, :years
    remove_column :subscription_plans, :cost_eu
    remove_column :subscription_plans, :cost_other
  end

  def down
    # Re-add the old columns in case of rollback
    add_column :subscription_plans, :years, :integer, default: 1, null: false
    add_column :subscription_plans, :cost_eu, :decimal, default: 0.0, null: false
    add_column :subscription_plans, :cost_other, :decimal, default: 0.0, null: false

    # Re-populate the old columns from the variants JSON column
    SubscriptionPlan.reset_column_information

    SubscriptionPlan.find_each do |record|
      # Extract data from variants JSON column
      variants_data = record.variants.first
      record.update_columns(years: variants_data['years'], cost_eu: variants_data['cost_eur'], cost_other: variants_data['cost_usd'])
    end

    # Remove the variants column
    remove_column :subscription_plans, :variants
  end
end
