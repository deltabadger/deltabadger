desc 'rake task to check time to first payment'
task update_saver_limits: :environment do
  all_times = []

  User.joins(:payments).includes(:payments).where(payments: { status: :paid }).find_each do |user|
    first_payment = user.payments.select(&:paid?).min_by(&:created_at)
    next if first_payment.nil?

    days_to_first_payment = ((first_payment.created_at - user.created_at) / 1.day).to_i
    puts "User #{user.id} took #{days_to_first_payment} days to first payment"
    all_times << days_to_first_payment
  end

  if all_times.any?
    Puts "From a total of #{all_times.size} first payments:"
    puts "Average time to first payment: #{(all_times.sum.to_f / all_times.size).to_i} days"
    puts "Median time to first payment: #{all_times.sort[all_times.size / 2]} days"
    puts "80th percentile time to first payment: #{all_times.sort[(all_times.size * 0.8).to_i]} days"
    puts "90th percentile time to first payment: #{all_times.sort[(all_times.size * 0.9).to_i]} days"
    puts "95th percentile time to first payment: #{all_times.sort[(all_times.size * 0.95).to_i]} days"
  else
    puts 'No payments found for any user.'
  end
end
