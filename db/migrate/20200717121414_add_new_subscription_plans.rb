class AddNewSubscriptionPlans < ActiveRecord::Migration[5.2]
  def change
    reversible do |dir|
      dir.up do
        execute <<~SQL
          UPDATE subscription_plans
            SET name = 'saver'
            WHERE name = 'free'
        SQL
        execute <<~SQL
          UPDATE subscription_plans
            SET name = 'investor'
            WHERE name = 'unlimited'
        SQL
        execute <<~SQL
          INSERT INTO subscription_plans (name, unlimited, years, cost_eu, cost_other, credits, created_at, updated_at)
            VALUES ('hodler', true, 4, 60, 60, 500, NOW(), NOW())
        SQL
      end

      dir.down do
        execute <<~SQL
          UPDATE subscription_plans
            SET name = 'free'
            WHERE name = 'saver'
        SQL
        execute <<~SQL
          UPDATE subscription_plans
            SET name = 'unlimited'
            WHERE name = 'investor'
        SQL
      end
    end
  end
end
