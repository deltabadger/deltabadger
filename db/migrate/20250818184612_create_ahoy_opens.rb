class CreateAhoyOpens < ActiveRecord::Migration[6.0]
  def change
    create_table :ahoy_opens do |t|
      t.string :campaign, index: true
      t.string :token
    end
  end
end
