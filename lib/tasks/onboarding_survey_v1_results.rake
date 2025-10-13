desc 'Generate onboarding survey V1 results'
task onboarding_survey_v1_results: :environment do
  answers = Survey.onboarding.pluck(:answers)

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

  answers.reject! { |answer| answer['preferred_exchange'].include?('binance') }
  answers.reject! { |answer| answer['preferred_exchange'].include?('coinbase') }
  answers.reject! { |answer| answer['preferred_exchange'].include?('kraken') }

  exchange_counts = answers.flat_map { |hash| hash['preferred_exchange'] }.tally

  puts "\nExchange counts (excluding Binance, Coinbase, Kraken):"
  exchange_counts.sort_by { |_, count| count }.reverse.each do |exchange, count|
    puts "#{exchange}: #{count} (#{(count.to_f / exchange_counts.values.sum * 100).round(2)}%)"
  end
end
