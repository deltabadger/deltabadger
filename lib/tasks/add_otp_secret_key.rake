desc 'rake task to update users with otp secret key'
task update_users_with_otp_secret_key: :environment do
  User.find_each do |user|
    user.update(otp_secret_key: ROTP::Base32.random_base32)
  end
end
