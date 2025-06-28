class AddSubtitleToArticles < ActiveRecord::Migration[6.0]
  def change
    add_column :articles, :subtitle, :string
  end
end
