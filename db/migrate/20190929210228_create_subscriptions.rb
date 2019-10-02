class CreateSubscriptions < ActiveRecord::Migration[5.2]
  def change
    create_table :subscriptions do |t|
      t.references :subscription_plan, foreign_key: true, index: true
      t.references :user, foreign_key: true, index: true
      t.datetime :end_time

      t.timestamps
    end
  end
end
