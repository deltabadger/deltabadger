class RenameSubscriptionPlans < ActiveRecord::Migration[6.0]
  def up
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'free'
        WHERE name = 'saver'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'standard'
        WHERE name = 'investor'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'pro'
        WHERE name = 'hodler'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'legendary'
        WHERE name = 'legendary_badger'
    SQL
  end

  def down
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'saver'
        WHERE name = 'free'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'investor'
        WHERE name = 'standard'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'hodler'
        WHERE name = 'pro'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'legendary_badger'
        WHERE name = 'legendary'
    SQL
  end
end
