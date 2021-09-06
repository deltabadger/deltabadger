desc 'Update selling bots'
task :update_selling_bots => :environment do
  ActiveRecord::Base.connection.execute("UPDATE bots SET settings = jsonb_set(settings, '{type}', '\"sell_old\"'::jsonb) WHERE settings->>'type' = 'sell'")
end