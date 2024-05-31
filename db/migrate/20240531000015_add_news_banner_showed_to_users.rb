class AddNewsBannerShowedToUsers < ActiveRecord::Migration[6.0]
  def change
    add_column :users, :news_banner_showed, :boolean, default: false
  end
end
