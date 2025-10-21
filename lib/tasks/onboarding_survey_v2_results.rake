desc 'Generate onboarding survey V2 results'
task onboarding_survey_v2_results: :environment do
  answers = Survey.onboarding_v2.pluck(:answers)

  exchange = answers.count { |answer| answer['investment_method'] == 'exchange' }
  defi = answers.count { |answer| answer['investment_method'] == 'defi' }

  puts "\nInvestment method counts:"
  puts "Exchange: #{exchange} (#{(exchange.to_f / answers.count * 100).round(2)}%)"
  puts "DeFi:     #{defi} (#{(defi.to_f / answers.count * 100).round(2)}%)"

  asset_counts = answers.flat_map { |hash| hash['investment_assets'] }.tally

  puts "\nInvestment asset counts:"
  asset_counts.sort_by { |_, count| count }.reverse.each do |asset, count|
    puts "#{asset}: #{count} (#{(count.to_f / answers.count * 100).round(2)}%)"
  end
end
