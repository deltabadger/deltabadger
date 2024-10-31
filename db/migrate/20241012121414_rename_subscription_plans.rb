class RenameSubscriptionPlans < ActiveRecord::Migration[6.0]
  def up
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'free'
        WHERE name = 'saver'
    SQL
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'basic'
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

    User.find_each do |user|
      user.update_columns(email: 'free@test.com') if user.email == 'saver@test.com'
      user.update_columns(email: 'basic@test.com') if user.email == 'investor@test.com'
      user.update_columns(email: 'pro@test.com') if user.email == 'hodler@test.com'
      user.update_columns(email: 'legendary@test.com') if user.email == 'legendary_badger@test.com'
    end
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
        WHERE name = 'basic'
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

    User.find_each do |user|
      user.update_columns(email: 'saver@test.com') if user.email == 'free@test.com'
      user.update_columns(email: 'investor@test.com') if user.email == 'basic@test.com'
      user.update_columns(email: 'hodler@test.com') if user.email == 'pro@test.com'
      user.update_columns(email: 'legendary_badger@test.com') if user.email == 'legendary@test.com'
    end
  end
end
