class AddOnboardingPreferencesToUsers < ActiveRecord::Migration[6.0]
  def change
    add_column :users, :onboarding_completed, :boolean, default: false
    add_column :users, :investment_goal, :string
    add_column :users, :preferred_exchange, :string
  end
end 