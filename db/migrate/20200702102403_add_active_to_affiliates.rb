class AddActiveToAffiliates < ActiveRecord::Migration[5.2]
  def change
    add_column :affiliates, :active, :boolean, null: false, default: true
  end
end
