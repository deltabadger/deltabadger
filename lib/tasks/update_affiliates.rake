desc 'one time rake task to add affiliate to user'
task :update_affiliates  => :environment do
  arr = []
  User.all.each do |user|
    unless user.affiliate.present?
      hash = {
        user_id: user.id,
        type: 'individual',
        discount_percent: 0.10,
        btc_address: nil,
        code: SecureRandom.hex(5),
        max_profit: Affiliate::DEFAULT_MAX_PROFIT,
        total_bonus_percent: Affiliate::DEFAULT_BONUS_PERCENT
      }
      arr.push(hash)
    end
  end

  Affiliate.create(arr)
end

