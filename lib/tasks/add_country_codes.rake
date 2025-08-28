desc 'rake task to add country codes'
task add_country_codes: :environment do
  Country.find_each do |country|
    case country.name
    when 'Austria' then country.update!(code: 'AT', currency: :eur, eu_member: true)
    when 'Belgium' then country.update!(code: 'BE', currency: :eur, eu_member: true)
    when 'Bulgaria' then country.update!(code: 'BG', currency: :eur, eu_member: true)
    when 'Croatia' then country.update!(code: 'HR', currency: :eur, eu_member: true)
    when 'Cyprus' then country.update!(code: 'CY', currency: :eur, eu_member: true)
    when 'Czechia' then country.update!(code: 'CZ', currency: :eur, eu_member: true)
    when 'Denmark' then country.update!(code: 'DK', currency: :eur, eu_member: true)
    when 'Estonia' then country.update!(code: 'EE', currency: :eur, eu_member: true)
    when 'Finland' then country.update!(code: 'FI', currency: :eur, eu_member: true)
    when 'France' then country.update!(code: 'FR', currency: :eur, eu_member: true)
    when 'Germany' then country.update!(code: 'DE', currency: :eur, eu_member: true)
    when 'Greece' then country.update!(code: 'GR', currency: :eur, eu_member: true)
    when 'Hungary' then country.update!(code: 'HU', currency: :eur, eu_member: true)
    when 'Ireland' then country.update!(code: 'IE', currency: :eur, eu_member: true)
    when 'Italy' then country.update!(code: 'IT', currency: :eur, eu_member: true)
    when 'Latvia' then country.update!(code: 'LV', currency: :eur, eu_member: true)
    when 'Lithuania' then country.update!(code: 'LT', currency: :eur, eu_member: true)
    when 'Luxembourg' then country.update!(code: 'LU', currency: :eur, eu_member: true)
    when 'Malta' then country.update!(code: 'MT', currency: :eur, eu_member: true)
    when 'Netherlands' then country.update!(code: 'NL', currency: :eur, eu_member: true)
    when 'Poland' then country.update!(code: 'PL', currency: :eur, eu_member: true)
    when 'Portugal' then country.update!(code: 'PT', currency: :eur, eu_member: true)
    when 'Romania' then country.update!(code: 'RO', currency: :eur, eu_member: true)
    when 'Slovakia' then country.update!(code: 'SK', currency: :eur, eu_member: true)
    when 'Slovenia' then country.update!(code: 'SI', currency: :eur, eu_member: true)
    when 'Spain' then country.update!(code: 'ES', currency: :eur, eu_member: true)
    when 'Sweden' then country.update!(code: 'SE', currency: :eur, eu_member: true)
    when 'Switzerland' then country.update!(code: 'CH', currency: :eur)
    end
  end
end
