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

  index_portfolios = %w[top_10_crypto top_5_crypto nasdaq_100 magnificent_7 sp_500 top_50_crypto altcoin_season]
  index_portfolio_count = answers.count { |answer| (answer['investment_assets'] & index_portfolios).any? }

  digital_stocks = %w[other_stocks nasdaq_100 magnificent_7 sp_500]
  digital_stocks_count = answers.count { |answer| (answer['investment_assets'] & digital_stocks).any? }

  crypto_only = %w[bitcoin ethereum top_10_crypto top_5_crypto other_crypto top_50_crypto altcoin_season]
  crypto_only_count = answers.count do |answer|
    (answer['investment_assets'] - crypto_only).empty? && answer['investment_assets'].any?
  end

  bitcoin_only_count = answers.count { |answer| answer['investment_assets'] == ['bitcoin'] }

  puts "\nSpecial categories:"
  puts "Index Portfolios: #{index_portfolio_count} (#{(index_portfolio_count.to_f / answers.count * 100).round(2)}%)"
  puts "Digital Stocks:   #{digital_stocks_count} (#{(digital_stocks_count.to_f / answers.count * 100).round(2)}%)"
  puts "Crypto Only:      #{crypto_only_count} (#{(crypto_only_count.to_f / answers.count * 100).round(2)}%)"
  puts "Bitcoin Only:     #{bitcoin_only_count} (#{(bitcoin_only_count.to_f / answers.count * 100).round(2)}%)"
end
