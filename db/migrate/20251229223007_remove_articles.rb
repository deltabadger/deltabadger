class RemoveArticles < ActiveRecord::Migration[6.0]
  def change
    drop_table :articles
    drop_table :authors
  end
end
