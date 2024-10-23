module UpgradeHelper
  def checkout_class_for(payment)
    payment.eu? ? 'db-checkout--eu' : 'db-checkout--other'
  end
end
