desc 'Generate onboarding survey results'
task onboarding_survey_results: :environment do
  answers = Survey.onboarding.pluck(:answers)

  # answers.reject! { |answer| answer['preferred_exchange'].include?('binance') }
  # answers.reject! { |answer| answer['preferred_exchange'].include?('coinbase') }
  # answers.reject! { |answer| answer['preferred_exchange'].include?('kraken') }

  buy_the_dip = answers.count { |answer| answer['investment_goal'] == 'buy_the_dip' }
  retire_early = answers.count { |answer| answer['investment_goal'] == 'retire_early' }

  puts "\nInvestment goal counts:"
  puts "Buy the dip:  #{buy_the_dip} (#{(buy_the_dip.to_f / answers.count * 100).round(2)}%)"
  puts "Retire early: #{retire_early} (#{(retire_early.to_f / answers.count * 100).round(2)}%)"

  exchange_counts = answers.flat_map { |hash| hash['preferred_exchange'] }.tally

  puts "\nExchange counts:"
  exchange_counts.sort_by { |_, count| count }.reverse.each do |exchange, count|
    puts "#{exchange}: #{count} (#{(count.to_f / exchange_counts.values.sum * 100).round(2)}%)"
  end
end
