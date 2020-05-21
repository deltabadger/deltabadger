class Exchange < ApplicationRecord
  def currencies
    case name.downcase
    when 'kraken' then %w[USD EUR CHF GBP CAD]
    when 'bitbay' then %w[USD EUR PLN]
    else
      %w[USD EUR]
    end
  end
end
