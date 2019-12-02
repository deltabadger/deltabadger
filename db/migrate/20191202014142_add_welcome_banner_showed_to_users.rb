class AddWelcomeBannerShowedToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :welcome_banner_showed, :boolean, default: false
  end
end
