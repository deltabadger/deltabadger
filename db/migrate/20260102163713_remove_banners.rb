class RemoveBanners < ActiveRecord::Migration[6.0]
  def change
    remove_column :users, :welcome_banner_dismissed
    remove_column :users, :news_banner_dismissed
  end
end
