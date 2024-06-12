class RenameBannerColumnsInUsers < ActiveRecord::Migration[6.0]
  def change
    rename_column :users, :welcome_banner_showed, :welcome_banner_dismissed
    rename_column :users, :news_banner_showed, :news_banner_dismissed
    rename_column :users, :referral_banner_showed, :referral_banner_dismissed
  end
end
