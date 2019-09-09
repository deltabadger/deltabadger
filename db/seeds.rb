Exchange.find_or_create_by!(name: 'Kraken')
Exchange.find_or_create_by!(name: 'Deribit')
Exchange.find_or_create_by!(name: 'BitBay')

User.find_or_create_by(
  email: "test@test.com"
) do |user|
  user.password = "polopolo"
  user.confirmed_at = user.confirmed_at || Time.now
end
