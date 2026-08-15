class CreateFundClassifications < ActiveRecord::Migration[8.1]
  def change
    create_table :fund_classifications do |t|
      t.references :user, null: false, foreign_key: true, index: false
      t.string :symbol, null: false
      t.string :isin
      t.integer :kind, null: false
      t.integer :fund_category
      t.timestamps
    end
    add_index :fund_classifications, %i[user_id symbol], unique: true
  end
end
