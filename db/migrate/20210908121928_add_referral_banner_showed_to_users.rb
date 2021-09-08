class AddReferralBannerShowedToUsers < ActiveRecord::Migration[5.2]
  def change
    add_column :users, :referral_banner_showed, :boolean, default: false
  end
end
