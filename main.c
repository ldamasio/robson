/*
 * Robson CLI - Historical entry point (v0.01+)
 *
 * This file is preserved for historical continuity.
 * It now acts as a THIN ROUTER that delegates all logic to robson-go.
 *
 * Design principle:
 *   Separate planning, validation, and execution at the CLI level,
 *   just as we separate idea formulation, validation, and execution in trading.
 */

#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <string.h>

/*
 * Translate legacy flags to modern subcommands.
 * Returns the normalized subcommand or NULL if no translation needed.
 */
static const char* translate_legacy_flag(const char* arg) {
	if (strcmp(arg, "--help") == 0) return "help";
	if (strcmp(arg, "--report") == 0) return "report";
	if (strcmp(arg, "--say") == 0) return "say";
	if (strcmp(arg, "--buy") == 0) return "buy";
	if (strcmp(arg, "--sell") == 0) return "sell";
	return NULL;
}

int main(int argc, char *argv[]) {
	if (argc < 2) {
		printf("Welcome to Robson 0.01\n");
		printf("Usage: robson <subcommand> [options]\n");
		printf("Try: robson help\n");
		return 0;
	}

	/* Build new argv for robson-go */
	char **new_argv = malloc(sizeof(char*) * (argc + 1));
	if (new_argv == NULL) {
		fprintf(stderr, "Memory allocation failed\n");
		return 1;
	}

	new_argv[0] = "robson-go";

	/* Translate legacy flag if present, otherwise pass through */
	const char* translated = translate_legacy_flag(argv[1]);
	if (translated != NULL) {
		new_argv[1] = (char*)translated;
		/* Copy remaining args */
		for (int i = 2; i < argc; i++) {
			new_argv[i] = argv[i];
		}
		new_argv[argc] = NULL;
	} else {
		/* Pass all args as-is (subcommand mode) */
		for (int i = 1; i < argc; i++) {
			new_argv[i] = argv[i];
		}
		new_argv[argc] = NULL;
	}

	/* Delegate to robson-go binary */
	execvp("robson-go", new_argv);

	/* If execvp returns, it failed */
	perror("Failed to execute robson-go");
	fprintf(stderr, "Make sure robson-go is installed and in your PATH\n");
	free(new_argv);
	return 1;
}
