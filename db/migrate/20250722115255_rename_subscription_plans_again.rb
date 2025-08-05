class RenameSubscriptionPlansAgain < ActiveRecord::Migration[6.0]
  def up
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'mini'
        WHERE name = 'basic'
    SQL

    User.find_each do |user|
      user.update_columns(email: 'mini@test.com') if user.email == 'basic@test.com'
    end
  end

  def down
    execute <<~SQL
      UPDATE subscription_plans
        SET name = 'basic'
        WHERE name = 'mini'
    SQL

    User.find_each do |user|
      user.update_columns(email: 'basic@test.com') if user.email == 'mini@test.com'
    end
  end
end
