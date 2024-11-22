# website table: https://support.kraken.com/hc/en-us/articles/360000767986-Cryptocurrency-withdrawal-fees-and-minimums

INPUT_FILE = 'app/services/exchange_api/withdrawal_info/kraken/website_table.html'.freeze
OUTPUT_FILE = 'app/services/exchange_api/withdrawal_info/kraken/kraken_minimums_and_fees.csv'.freeze
# MAIN_CHAIN_FOR_MULTIPLE_CHAINS_ASSETS = {
#   'XBT' => '',
#   'ETH' => '',
#   'DAI' => '(Ethereum)',
#   'ENJ' => '(Enjin Relaychain)',
#   'XLM' => '(Stellar - Memo required)',
#   'PYUSD' => '(Ethereum)',
#   'POL' => '(Polygon - Standard)',
#   'USDT' => '(Ethereum)',
#   'USDC' => '(Ethereum)'
# }.freeze

desc 'Convert the Kraken HTML table to a CSV file'
task update_kraken_minimums_and_fees_csv: :environment do
  require 'nokogiri'
  require 'csv'

  unless File.exist?(INPUT_FILE)
    puts "Input file not found: #{INPUT_FILE}"
    exit
  end

  html_content = File.read(INPUT_FILE)
  doc = Nokogiri::HTML(html_content)

  table = doc.at('table')
  unless table
    puts 'No table found in the HTML file.'
    exit
  end

  headers = %w[Asset Minimum Fee]
  rows = table.css('tr').drop(1).map do |row|
    row.css('td').map { |td| td.text.strip }.then do |cols|
      symbol = cols.last.split.last.gsub(/\bBTC\b/, 'XBT')
      chain = get_chain(cols[0])
      cols[0] = symbol + (chain ? " #{chain}" : '')
      # cols[1], cols[2] = cols[2].split.first, cols[1].split.first
      cols[1], cols[2] = [cols[2].split.first.to_f * 2, cols[1].split.first.to_f * 10].min, cols[1].split.first
      cols
    end
  end

  symbols_to_skip = []
  rows = rows.map do |row|
    asset = row[0]
    symbol = asset.split.first
    next unless specific_chain?(asset)

    row[0] = symbol

    # # OPTION 1: use a specific chain if it exists
    # all_assets = rows.map { |row| row[0] }
    # all_chains_for_symbol = all_assets.map { |a| a.split.first == symbol ? get_chain(a) : nil }.compact
    # if all_chains_for_symbol.size > 1
    #   if MAIN_CHAIN_FOR_MULTIPLE_CHAINS_ASSETS.include?(symbol)
    #     next unless MAIN_CHAIN_FOR_MULTIPLE_CHAINS_ASSETS[symbol] == get_chain(asset)
    #   else
    #     puts "WARNING! Multiple chains found for #{asset}, this asset has been ignored. Please add it to the MAIN_CHAIN_FOR_MULTIPLE_CHAINS_ASSETS constant and run the task again." # rubocop:disable Layout/LineLength
    #     next
    #   end
    # end
    # # OPTION 1: use a specific chain if it exists

    # OPTION 2: use the most restrictive chain found
    next if symbols_to_skip.include?(symbol)

    all_rows_for_symbol = rows.map { |r| r[0].split.first == symbol ? r : nil }.compact
    if all_rows_for_symbol.size > 1
      row[1] = all_rows_for_symbol.map { |r| r[1].to_f }.max
      row[2] = all_rows_for_symbol.map { |r| r[2].to_f }.max
      symbols_to_skip << symbol
    end
    # OPTION 2: use the most restrictive chain found

    row
  end.uniq.compact

  table_hash = rows.sort.map do |row|
    headers.zip(row).to_h
  end

  CSV.open(OUTPUT_FILE, 'w') do |csv|
    csv << headers
    table_hash.each do |row|
      csv << row.values
    end
  end

  puts "CSV file created successfully at #{OUTPUT_FILE}"
end

def get_chain(string)
  chain_starts_at_character = string.index('(') # match the chain name in parentheses
  return unless chain_starts_at_character

  string[chain_starts_at_character..].gsub(',', ' -').gsub('*', '').to_s
end

def specific_chain?(string)
  string.include?('(')
end
