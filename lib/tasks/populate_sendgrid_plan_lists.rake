desc 'rake task to repopulate Sendgrid plan lists'
task populate_sendgrid_plan_lists: :environment do
  data = {}
  puts "Processing #{User.count} users"
  User.find_each do |user|
    subscription_name = user.subscription&.name
    next if subscription_name.blank?

    data[subscription_name] ||= []
    email = user.email
    name = user.name.split.first.capitalize if user.name.present?
    contact = {
      email: email,
      first_name: name
    }.compact
    data[subscription_name] << contact
  end

  data.each do |subscription_name, _contacts|
    list_name = ENV.fetch("SENDGRID_#{subscription_name.upcase}_USERS_LIST")
    list_id = get_list_id(list_name)
    raise StandardError, "List '#{list_name}' not found" if list_id.nil?

    contacts = data[subscription_name]
    puts "Adding #{contacts.size} contacts to list '#{list_name}'"
    result = client.add_or_update_contacts(list_ids: [list_id], contacts: contacts)
    raise StandardError, result.errors if result.failure?
  end

  puts 'Done!'
end

# same as SendgridJob:

def client
  @client ||= SendgridClient.new
end

def get_list_id(list_name)
  result = client.get_all_lists
  result.data.fetch('result').select { |list| list['name'] == list_name }.pluck('id').first
end
