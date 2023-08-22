from django_multitenant.utils import set_current_tenant, unset_current_tenant
from django.contrib.auth import logout


class MultitenantMiddleware:
    def __init__(self, get_response):
        self.get_response = get_response

    def __call__(self, request):
        if request.user and not request.user.is_anonymous:
            if not request.user.account and not request.user.is_superuser:
                print(
                    "Logging out because user doesnt have account and not a superuser"
                )
                logout(request.user)

            set_current_tenant(request.user.account)

        response = self.get_response(request)

        """
     The following unsetting of the tenant is essential because of how webservers work
     Since the tenant is set as a thread local, the thread is not killed after the request is processed
     So after processing of the request, we need to ensure that the tenant is unset
     Especially required if you have public users accessing the site

     This is also essential if you have admin users not related to a tenant (not possible in actual citus env)
     """
        unset_current_tenant()

        return response
