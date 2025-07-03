class AddSocialUrlsToArticles < ActiveRecord::Migration[6.0]
  def change
    add_column :articles, :x_url, :string
    add_column :articles, :telegram_url, :string
  end
end
