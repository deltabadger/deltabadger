desc 'Get the percentage of a bots that are in profit.'
task get_profit_percentage: [:environment] do
  MetricsRepository.new.profitable_bots_data
end
